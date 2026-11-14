// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-billingconductor-awsbillingconductor service.
//!
//! The pricing-rule / pricing-plan / billing-group core loop is persisted here.
//! Every record is keyed by its FULL ARN, because that is the identity the wire
//! uses: the modeled `BillingGroupArn` / `PricingPlanArn` / `PricingRuleArn`
//! patterns accept either the full ARN or the bare resource id, so a handler
//! normalises its input through [`normalize_billing_group_arn`] /
//! [`normalize_pricing_plan_arn`] / [`normalize_pricing_rule_arn`] before touching
//! the maps and two spellings of one resource cannot become two records.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
mod refusals;
mod billing_period_checks;
mod pagination;
mod tagging;
pub use tagging::*;
pub use pagination::*;
pub use billing_period_checks::*;
pub use refusals::*;

/// This service's ARN namespace (`OPERATIONS.md`: "ARN namespace: billingconductor").
const ARN_SERVICE: &str = "billingconductor";

/// billingconductor is a global (billing) service, so its ARNs carry no region —
/// the modeled patterns have an empty region segment
/// (`arn:aws:billingconductor::<account>:billinggroup/...`).
const ARN_REGION: &str = "";

/// The pricing plans Amazon Web Services owns rather than the caller: the
/// `PricingPlanFullArn` pattern enumerates them beside the 10-character
/// customer-minted id
/// (`.../pricingplan/(BasicPricingPlan|Passthrough|[a-zA-Z0-9]{10})`), so a billing
/// group may name one without the caller ever having created it.
pub const AWS_MANAGED_PRICING_PLAN_IDS: &[&str] = &["BasicPricingPlan", "Passthrough"];

/// The alphabet the modeled resource-id patterns allow (`[a-zA-Z0-9]`).
const ID_ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// Wall-clock millis since the epoch — the unit the modeled `Instant` long carries
/// (`CreationTime` / `LastModifiedTime`).
pub fn now_millis() -> i64 {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
}

/// The full ARN of a pricing plan resource id.
pub fn pricing_plan_arn(id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, ARN_REGION, &format!("pricingplan/{id}"))
}

/// The full ARN of a billing group resource id.
pub fn billing_group_arn(id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, ARN_REGION, &format!("billinggroup/{id}"))
}

/// The full ARN of a custom line item resource id.
pub fn custom_line_item_arn(id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, ARN_REGION, &format!("customlineitem/{id}"))
}

/// The full ARN of a pricing rule resource id.
pub fn pricing_rule_arn(id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, ARN_REGION, &format!("pricingrule/{id}"))
}

/// A `PricingRuleArn` as this service stores it: the pattern accepts the full ARN
/// or the bare `[a-zA-Z0-9]{10}` id, and both name the same resource. There are no
/// AWS-managed pricing rules — the `PricingRuleArn` pattern allows only a 12-digit
/// account segment — so a bare id always resolves into this account.
pub fn normalize_pricing_rule_arn(value: &str) -> String {
    if value.starts_with("arn:") {
        value.to_string()
    } else {
        pricing_rule_arn(value)
    }
}

/// A `BillingGroupArn` as this service stores it: the pattern accepts the full ARN
/// or the bare `[a-zA-Z0-9]{10,12}` id, and both name the same resource.
pub fn normalize_billing_group_arn(value: &str) -> String {
    if value.starts_with("arn:") {
        value.to_string()
    } else {
        billing_group_arn(value)
    }
}

/// A `CustomLineItemArn` as this service stores it: the pattern accepts the full ARN
/// or the bare `[a-zA-Z0-9]{10}` id. See [`normalize_billing_group_arn`].
pub fn normalize_custom_line_item_arn(value: &str) -> String {
    if value.starts_with("arn:") {
        value.to_string()
    } else {
        custom_line_item_arn(value)
    }
}

/// How a resource associated to a percentage custom line item relates to it.
///
/// `CustomLineItemAssociationElement` admits a `billinggroup` ARN or a
/// `customlineitem` ARN, and `CustomLineItemRelationship` has exactly two values —
/// one per class: the billing group a custom line item is computed inside sits ABOVE
/// it (`PARENT`), and another custom line item whose charge feeds the percentage sits
/// BELOW it (`CHILD`). So the resource type segment of the ARN decides, and the
/// `Relationship` filter separates the two classes.
pub fn association_relationship(arn: &str) -> &'static str {
    if arn.contains(":billinggroup/") {
        "PARENT"
    } else {
        "CHILD"
    }
}

/// A `BillingPeriod` (`^\d{4}-(0?[1-9]|1[012])$`) as a comparable month count, so
/// `2026-7` and `2026-07` are the same period and two periods order by time.
/// `None` when the value is not a billing period at all.
///
/// The shape of each segment is checked HERE rather than assumed: this is the point
/// that decides whether a caller-supplied string is a billing period, so the year is
/// exactly four ASCII digits and the month one or two — which also keeps the month
/// count far inside `i64`, whatever a request carries.
pub fn billing_period_key(value: &str) -> Option<i64> {
    let (year, month) = value.split_once('-')?;
    if year.len() != 4 || !year.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    if !(1..=2).contains(&month.len()) || !month.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    let year: i64 = year.parse::<u16>().ok()?.into();
    let month: i64 = month.parse::<u8>().ok()?.into();
    (1..=12).contains(&month).then_some(year * 12 + month)
}

/// The CURRENT billing period: the month a billing read uses when the request names
/// none (`ListCustomLineItems`: "If you don't provide a billing period, the current
/// billing period is used").
///
/// Built from the SAME clock as [`now_millis`] and rendered in the form
/// `BillingPeriod`'s pattern accepts (`^\d{4}-(0?[1-9]|1[012])$`), so it is a value a
/// caller could have sent and [`billing_period_key`] reads it back. Billing is
/// reported in UTC, so the month is the UTC one.
///
/// A `BillingPeriod` is exactly the `<year>-<month>` head of the ISO 8601 date-time
/// (`YYYY-MM-DDThh:mm:ssZ`) the SAME instant renders as, so the calendar conversion is
/// `aws_smithy_types::DateTime` — the formatter this emulator already serializes its
/// `@timestampFormat("date-time")` members with — rather than a second date library or
/// hand-written civil-calendar arithmetic.
pub fn current_billing_period() -> String {
    billing_period_at(now_millis())
}

/// The `BillingPeriod` the instant `millis` falls in — the month a call made then
/// bills in.
///
/// The calendar conversion is the one [`current_billing_period`] describes, applied to
/// an instant the store holds rather than to the clock, so the period a stored write
/// took effect in is derivable from the instant it was recorded at and no second
/// spelling of a month is persisted beside it.
pub fn billing_period_at(millis: i64) -> String {
    ::aws_smithy_types::DateTime::from_millis(millis)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .and_then(|rendered| rendered.get(.."YYYY-MM".len()).map(str::to_string))
        // Unreachable for a clock the formatter accepts; the epoch month is still a
        // billing period, so a read is answered rather than failed.
        .unwrap_or_else(|| "1970-01".to_string())
}

/// Is `billing_period` inside the half-open range `[start, end)` a custom line item or
/// a resource association carries?
///
/// The member names carry the rule — `InclusiveStartBillingPeriod` is in the range and
/// `ExclusiveEndBillingPeriod` is not — and an ABSENT bound is open. A `billing_period`
/// that is not a billing period at all covers nothing.
///
/// A STORED resource never carries an absent inclusive start: a resource is in effect no
/// earlier than the billing period the call that minted it was made in, so every record
/// here is built through [`BillingPeriodSpan::started_now`], which fills that bound in.
/// The open-start case is left to this predicate because a REMOVAL range, which bounds
/// what a call takes away rather than when a resource exists, is still open below.
pub fn billing_period_range_covers(
    inclusive_start: Option<&str>,
    exclusive_end: Option<&str>,
    billing_period: &str,
) -> bool {
    let Some(wanted) = billing_period_key(billing_period) else {
        return false;
    };
    let after_start = inclusive_start
        .and_then(billing_period_key)
        .is_none_or(|start| wanted >= start);
    let before_end = exclusive_end
        .and_then(billing_period_key)
        .is_none_or(|end| wanted < end);
    after_start && before_end
}

/// The billing period a comparable month count ([`billing_period_key`]) names, in the
/// canonical `YYYY-MM` spelling [`billing_period_at`] renders.
///
/// The inverse of [`billing_period_key`], whose count is `year * 12 + month` with the
/// month in `1..=12`, so the year is the count one month back divided by twelve and the
/// month is what is left of it.
fn billing_period_of_key(key: i64) -> String {
    let year = (key - 1).div_euclid(12);
    let month = key - year * 12;
    format!("{year:04}-{month:02}")
}

/// The billing periods of the half-open range `[inclusive_start, exclusive_end)`, in
/// ascending order and each in the canonical `YYYY-MM` spelling — the periods a report
/// over that range covers.
///
/// The start is in the range and the end is not, as the member names of a
/// `BillingPeriodRange` say. The periods are ENUMERATED from the range rather than read
/// off a stored value, so their spelling is canonical however the request spelled the
/// bounds, and the list is cut at [`MAX_BILLING_PERIODS_PER_RANGE`] so a caller cannot
/// ask one read to walk an unbounded number of months. A bound that is not a billing
/// period, or a range that covers none, enumerates nothing.
pub fn billing_periods_in_range(inclusive_start: &str, exclusive_end: &str) -> Vec<String> {
    let (Some(start), Some(end)) = (
        billing_period_key(inclusive_start),
        billing_period_key(exclusive_end),
    ) else {
        return Vec::new();
    };
    (start..end)
        .take(MAX_BILLING_PERIODS_PER_RANGE)
        .map(billing_period_of_key)
        .collect()
}

/// How many billing periods one `BillingPeriodRange` may cover: "You can specify up to
/// 12 months" (`GetBillingGroupCostReportInput.BillingPeriodRange`).
pub const MAX_BILLING_PERIODS_PER_RANGE: usize = 12;

/// How many billing periods the half-open range `[inclusive_start, exclusive_end)`
/// covers, or `None` when either bound is not a billing period.
pub fn billing_period_range_length(inclusive_start: &str, exclusive_end: &str) -> Option<i64> {
    let start = billing_period_key(inclusive_start)?;
    let end = billing_period_key(exclusive_end)?;
    Some(end - start)
}

/// Is a half-open range ordered, i.e. does it cover anything at all?
///
/// The bound names carry the rule on every axis the crate orders — billing periods,
/// usage: the INCLUSIVE lower bound is in the range and the EXCLUSIVE upper bound is
/// not, so the range covers something only when the upper bound comes strictly after
/// the lower one. An absent lower bound is open below and an absent upper bound open
/// above, so either one absent orders.
pub fn half_open_range_is_ordered<T: PartialOrd>(
    inclusive_begin: Option<T>,
    exclusive_end: Option<T>,
) -> bool {
    match (inclusive_begin, exclusive_end) {
        (Some(begin), Some(end)) => begin < end,
        _ => true,
    }
}

/// Do two half-open ranges of one axis cover a common point?
///
/// Exactly when EACH of them begins strictly before the other ends, which is
/// [`half_open_range_is_ordered`] read across the two ranges, an absent bound being
/// open as it is there.
pub fn half_open_ranges_overlap<T: PartialOrd>(
    (first_begin, first_end): (Option<T>, Option<T>),
    (second_begin, second_end): (Option<T>, Option<T>),
) -> bool {
    half_open_range_is_ordered(first_begin, second_end)
        && half_open_range_is_ordered(second_begin, first_end)
}

/// Does the half-open bound `start` come strictly before `end`? An absent `start` is
/// the unbounded past and an absent `end` the unbounded future, so either one absent
/// orders. Both bounds are the month counts [`billing_period_key`] returns.
///
/// The billing-period axis of the crate's one half-open comparison,
/// [`half_open_range_is_ordered`].
pub fn billing_period_precedes(start: Option<i64>, end: Option<i64>) -> bool {
    half_open_range_is_ordered(start, end)
}

/// One half-open span of billing periods, `[InclusiveStartBillingPeriod,
/// ExclusiveEndBillingPeriod)`, as a `CustomLineItemBillingPeriodRange` names one and
/// as [`billing_period_range_covers`] reads it: an ABSENT bound is open.
///
/// A span a STORED resource is in effect over is built through [`Self::started_now`], so
/// its inclusive start is always a real billing period; an open start is left to the
/// ranges that describe what a call REMOVES.
///
/// The bounds are kept as the STRINGS a request carried, because a billing period has
/// two spellings of one month (`2026-7` and `2026-07`) and a read reports the period
/// back as the call that SET it named it — a later call that only restates the same
/// month in the other spelling sets nothing, so it leaves the spelling alone; ordering
/// goes through [`billing_period_key`], so the two spellings still compare equal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BillingPeriodSpan {
    pub inclusive_start: Option<String>,
    pub exclusive_end: Option<String>,
}

impl BillingPeriodSpan {
    /// A span of the two bounds a `BillingPeriodRange` carries.
    pub fn new(inclusive_start: Option<String>, exclusive_end: Option<String>) -> Self {
        Self {
            inclusive_start,
            exclusive_end,
        }
    }

    /// The span a resource MINTED NOW is in effect over, given the bounds the minting
    /// call named.
    ///
    /// A resource is in effect no earlier than the billing period the call that minted it
    /// was made in, because it did not exist in any period before that one and a closed
    /// bill never acquires a resource that was not in it. So a call that named NO
    /// inclusive start begins in the CURRENT billing period rather than in the unbounded
    /// past — the bound every billing-period-scoped record this service stores carries
    /// ([`PricingRuleAssociationRecord::started_now`]). An inclusive start the call did
    /// name is kept as the call spelled it, and an absent END stays open: nothing has
    /// ended the resource, so it stands in every later period.
    pub fn started_now(inclusive_start: Option<String>, exclusive_end: Option<String>) -> Self {
        Self::new(
            Some(inclusive_start.unwrap_or_else(current_billing_period)),
            exclusive_end,
        )
    }

    /// The span a call that ENDS a resource NOW takes away from it: the billing period
    /// the call was made in and every later one.
    ///
    /// A call that stops a billing-period-scoped record standing, and names no range of
    /// its own, takes effect FROM the period it is made in and never earlier — the
    /// periods before it are closed bills the record really was in effect in, so a read
    /// scoped to one of them still reports it. That is the bound the rest of this store
    /// ends a record at ([`ComAmazonawsBillingconductorAwsbillingconductorState::release_account`],
    /// [`PricingPlanRecord::end_rule_associations`]), and the counterpart of
    /// [`Self::started_now`]: an absent END stays open, because nothing puts the record
    /// back in effect in a later period.
    pub fn ending_now() -> Self {
        Self::new(Some(current_billing_period()), None)
    }

    /// Do this span and `other` cover the SAME billing periods?
    ///
    /// A billing period has two spellings of one month (`2026-7` and `2026-07`), so the
    /// bounds are compared as the month counts [`billing_period_key`] reads them — the
    /// axis every other ordering here uses — rather than as the strings a request
    /// carried: two spans a read reports the resource for in the very same periods cover
    /// the same periods, however the call that set them spelled their bounds.
    pub fn covers_same_periods_as(&self, other: &Self) -> bool {
        self.start_key() == other.start_key() && self.end_key() == other.end_key()
    }

    /// Is `billing_period` inside this span? See [`billing_period_range_covers`].
    pub fn covers(&self, billing_period: &str) -> bool {
        billing_period_range_covers(
            self.inclusive_start.as_deref(),
            self.exclusive_end.as_deref(),
            billing_period,
        )
    }

    /// This span's inclusive start as a comparable month count; `None` when it is open.
    fn start_key(&self) -> Option<i64> {
        self.inclusive_start.as_deref().and_then(billing_period_key)
    }

    /// This span's exclusive end as a comparable month count; `None` when it is open.
    fn end_key(&self) -> Option<i64> {
        self.exclusive_end.as_deref().and_then(billing_period_key)
    }

    /// The billing periods of this span that are NOT in `removed`, as spans in
    /// ascending order.
    ///
    /// Subtracting one half-open span from another leaves the part BEFORE the removal
    /// starts and the part FROM the removal's end on — so a removal that is bounded on
    /// both sides and falls strictly inside this span leaves two, a removal that
    /// overlaps one end leaves one, and a removal that covers this span leaves none.
    /// A removal that shares no billing period with this span leaves it as it is.
    pub fn minus(&self, removed: &Self) -> Vec<Self> {
        let (start, end) = (self.start_key(), self.end_key());
        let (removed_start, removed_end) = (removed.start_key(), removed.end_key());
        // Two half-open spans share a billing period only when each one starts before
        // the other ends.
        if !billing_period_precedes(start, removed_end)
            || !billing_period_precedes(removed_start, end)
        {
            return vec![self.clone()];
        }
        let mut left = Vec::new();
        if removed_start.is_some() && billing_period_precedes(start, removed_start) {
            left.push(Self::new(
                self.inclusive_start.clone(),
                removed.inclusive_start.clone(),
            ));
        }
        if removed_end.is_some() && billing_period_precedes(removed_end, end) {
            left.push(Self::new(
                removed.exclusive_end.clone(),
                self.exclusive_end.clone(),
            ));
        }
        left
    }

    /// This span and `other` as ONE span, when together they cover an unbroken stretch
    /// of billing periods; `None` when a period neither of them covers lies between
    /// them, so the two are separate stretches.
    ///
    /// Two half-open spans join when neither one's exclusive end falls before the
    /// other's inclusive start: they overlap, or one begins exactly where the other
    /// ends, which is contiguous because the end is EXCLUSIVE and so names the first
    /// period the span does NOT cover. The joined span runs from the earlier inclusive
    /// start to the later exclusive end, each bound kept as the call that set it spelled
    /// it, and an absent bound is open and so swallows the other.
    ///
    /// The counterpart of [`Self::minus`]: subtraction is what a call that ends a
    /// resource over a range does to its spans, and this is what a call that puts it in
    /// effect over one does ([`add_billing_periods`]).
    pub fn union(&self, other: &Self) -> Option<Self> {
        let (start, end) = (self.start_key(), self.end_key());
        let (other_start, other_end) = (other.start_key(), other.end_key());
        if periods_lie_between(end, other_start) || periods_lie_between(other_end, start) {
            return None;
        }
        let inclusive_start = match (start, other_start) {
            (Some(start), Some(other_start)) if other_start < start => {
                other.inclusive_start.clone()
            }
            // An absent start is the unbounded past, so the other span's start is the
            // earlier one only when this span has one at all.
            (Some(_), None) => other.inclusive_start.clone(),
            _ => self.inclusive_start.clone(),
        };
        let exclusive_end = match (end, other_end) {
            (Some(end), Some(other_end)) if other_end > end => other.exclusive_end.clone(),
            // An absent end is open, so it is the later one.
            (Some(_), None) => other.exclusive_end.clone(),
            _ => self.exclusive_end.clone(),
        };
        Some(Self::new(inclusive_start, exclusive_end))
    }

    /// The billing periods this span and `other` BOTH cover, as one span; `None` when
    /// they share no billing period at all.
    ///
    /// Two half-open spans share a period exactly when each one begins strictly before
    /// the other ends ([`billing_period_precedes`] read across the two, as [`Self::minus`]
    /// reads it), and the periods they share run from
    /// the LATER of the two inclusive starts to the EARLIER of the two exclusive ends —
    /// each bound kept as the call that set it spelled it, and an absent bound being open
    /// and so never the tighter one.
    ///
    /// The third of the span operations [`Self::minus`] and [`Self::union`] belong to:
    /// subtraction is what a call that ends a resource over a range does to its spans,
    /// union what a call that puts it in effect over one does, and this is what BOUNDING
    /// a range to the periods another resource is there for does
    /// ([`intersect_billing_periods`]).
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let (start, end) = (self.start_key(), self.end_key());
        let (other_start, other_end) = (other.start_key(), other.end_key());
        if !billing_period_precedes(start, other_end) || !billing_period_precedes(other_start, end)
        {
            return None;
        }
        let inclusive_start = match (start, other_start) {
            (Some(start), Some(other_start)) if other_start > start => {
                other.inclusive_start.clone()
            }
            // An absent start is the unbounded past, so the other span's start is the
            // later one whenever it has one at all.
            (None, Some(_)) => other.inclusive_start.clone(),
            _ => self.inclusive_start.clone(),
        };
        let exclusive_end = match (end, other_end) {
            (Some(end), Some(other_end)) if other_end < end => other.exclusive_end.clone(),
            // An absent end is open, so the other span's end is the earlier one whenever
            // it has one at all.
            (None, Some(_)) => other.exclusive_end.clone(),
            _ => self.exclusive_end.clone(),
        };
        Some(Self::new(inclusive_start, exclusive_end))
    }
}

/// Is there a billing period between the half-open bound `end` and the inclusive bound
/// `start` — does `end` fall strictly before `start`?
///
/// Both are the month counts [`billing_period_key`] returns, and an ABSENT bound is
/// open — the unbounded future or past — so nothing lies beyond it.
fn periods_lie_between(end: Option<i64>, start: Option<i64>) -> bool {
    matches!((end, start), (Some(end), Some(start)) if end < start)
}

/// Take the billing periods `removed` names out of `spans`, the stretches of billing
/// periods ONE resource is in effect over, and report what that did to them.
///
/// Each span keeps whatever of itself lies outside the removal
/// ([`BillingPeriodSpan::minus`]), so a removal bounded on both sides takes only the
/// periods between its bounds and the resource resumes at the removal's exclusive end,
/// while a removal sharing no period with any span leaves them all as they are. The
/// spans stay disjoint and ascending, because subtracting one span from an ascending
/// disjoint list leaves one.
///
/// A removal that would carve the resource into more than [`MAX_BILLING_PERIOD_SPANS`]
/// separate stretches is REFUSED and writes nothing, because a caller decides how many
/// such calls to make and each one can add a stretch.
///
/// Every store that holds the billing periods one resource is in effect in, a resource
/// association ([`ResourceAssociationRecord::remove_periods`]) as much as a custom line
/// item ([`CustomLineItemRecord::remove_periods`]), removes periods THROUGH this
/// function, so a bounded call means the same thing wherever it lands.
pub fn remove_billing_periods(
    spans: &mut Vec<BillingPeriodSpan>,
    removed: &BillingPeriodSpan,
) -> PeriodsRemoval {
    let left: Vec<BillingPeriodSpan> = spans.iter().flat_map(|span| span.minus(removed)).collect();
    if left == *spans {
        return PeriodsRemoval::Unchanged;
    }
    if left.len() > MAX_BILLING_PERIOD_SPANS {
        return PeriodsRemoval::AtSpanLimit;
    }
    *spans = left;
    PeriodsRemoval::Changed
}

/// Put the billing periods `added` names INTO `spans`, the stretches of billing periods
/// ONE resource is in effect over, and report what that did to them.
///
/// The periods a resource is in effect over are the UNION of the ranges the calls that
/// put it in effect named: a range that runs on from a stretch the resource already has
/// — overlapping it, or beginning exactly at its exclusive end — joins that stretch into
/// one ([`BillingPeriodSpan::union`]), a range sharing no period with any of them becomes
/// a stretch of its own, and a range every period of which is already in effect leaves
/// them exactly as they are, so a repeat of one call changes nothing. The spans stay
/// disjoint and ascending, because joining every stretch a span touches leaves one.
///
/// An addition that would leave the resource in more separate stretches than
/// [`MAX_BILLING_PERIOD_SPANS`] is REFUSED and writes nothing, for the reason that
/// constant gives.
///
/// The inverse of [`remove_billing_periods`], and the only place billing periods are put
/// into a stored span list, so a bounded call means the same thing wherever it lands.
pub fn add_billing_periods(
    spans: &mut Vec<BillingPeriodSpan>,
    added: &BillingPeriodSpan,
) -> PeriodsAddition {
    let left = joined_billing_periods(spans, added);
    if left == *spans {
        return PeriodsAddition::Unchanged;
    }
    if left.len() > MAX_BILLING_PERIOD_SPANS {
        return PeriodsAddition::AtSpanLimit;
    }
    *spans = left;
    PeriodsAddition::Changed
}

/// `spans` with `added` joined in: the fewest disjoint ascending stretches that cover
/// every billing period either side covers.
///
/// The joining rule [`add_billing_periods`] writes through, without its bound, so a
/// read that only has to KNOW the periods a resource is in effect over — rather than
/// store them — answers from the same arithmetic the writer uses
/// ([`CustomLineItemRecord::applied_periods`]).
fn joined_billing_periods(
    spans: &[BillingPeriodSpan],
    added: &BillingPeriodSpan,
) -> Vec<BillingPeriodSpan> {
    let mut joined = added.clone();
    let mut left: Vec<BillingPeriodSpan> = Vec::new();
    for span in spans {
        match span.union(&joined) {
            Some(union) => joined = union,
            None => left.push(span.clone()),
        }
    }
    left.push(joined);
    // The stretches a read walks are ascending, and the one this call joined can belong
    // anywhere among them; an absent inclusive start is the unbounded past, so it sorts
    // first.
    left.sort_by_key(|span| span.start_key().unwrap_or(i64::MIN));
    left
}

/// The billing periods of `within` that `spans` also cover, as spans in the order
/// `spans` holds them — `within` BOUNDED to the periods `spans` are in effect over.
///
/// One span at a time ([`BillingPeriodSpan::intersection`]), so a `spans` list that is
/// disjoint and ascending leaves one; empty when `within` shares no billing period with
/// any of them, which is a range bounded down to no period at all.
pub fn intersect_billing_periods(
    spans: &[BillingPeriodSpan],
    within: &BillingPeriodSpan,
) -> Vec<BillingPeriodSpan> {
    spans
        .iter()
        .filter_map(|span| span.intersection(within))
        .collect()
}

/// The `CurrencyCode` a resource's charges are denominated in, DERIVED from the
/// PARTITION of its ARN.
///
/// `CurrencyCode` has exactly two values
/// ([`crate::ops::types::CURRENCY_CODE_VALUES`]: `CNY`, `USD`) and the modelled charge
/// value is documented "in USD" (`ListCustomLineItemFlatChargeDetails.ChargeValue`),
/// while the only other partition this service's ARN patterns admit is China
/// (`arn:aws-cn`), which bills in CNY. So the partition the resource lives in decides,
/// and nothing stores a currency beside the resource.
pub fn partition_currency_code(arn: &str) -> &'static str {
    if arn.starts_with("arn:aws-cn:") {
        "CNY"
    } else {
        "USD"
    }
}

/// The `CurrencyCode` a custom line item's charge value is denominated in — the
/// currency of the partition it lives in, see [`partition_currency_code`].
pub fn custom_line_item_currency_code(arn: &str) -> &'static str {
    partition_currency_code(arn)
}

/// The product a custom line item's charges are presented under in the bill.
///
/// `PresentationDetails.Service` is "the service under which the custom line item
/// charges will be presented", so an item that carries one is presented under it. An
/// item that carries none is presented under the service that raises the charge, which
/// is this one — the name its own model gives it
/// (`smithy_query(query="service_info")`: `AWSBillingConductor`) — so the product of a
/// charge is always known and never invented per request.
pub fn custom_line_item_product_name(item: &CustomLineItemRecord) -> &str {
    item.presentation_service
        .as_deref()
        .unwrap_or(BILLING_CONDUCTOR_PRODUCT_NAME)
}

/// The product name of this service's own charges — see
/// [`custom_line_item_product_name`].
pub const BILLING_CONDUCTOR_PRODUCT_NAME: &str = "AWSBillingConductor";

/// The sign a `CustomLineItemChargeDetails.Type` gives a charge on a bill: a `FEE` adds
/// to the bill and a `CREDIT` comes off it ("The type of the custom line item that
/// indicates whether the charge is a fee or credit";
/// [`crate::ops::types::CUSTOM_LINE_ITEM_TYPE_VALUES`] has exactly those two values).
pub fn charge_sign(charge_type: &str) -> f64 {
    if charge_type == CREDIT_CHARGE_TYPE {
        -1.0
    } else {
        1.0
    }
}

/// The `CustomLineItemType` that takes its value OFF the bill rather than adding it.
const CREDIT_CHARGE_TYPE: &str = "CREDIT";

/// The actual Amazon Web Services charges this emulator has recorded for a billing
/// group's accounts: NONE.
///
/// `AWSCost` is "the actual Amazon Web Services charges for the billing group" — the
/// metered usage of its member accounts. Nothing in this service's surface reports usage
/// into the store (its writes create billing groups, pricing plans, pricing rules and
/// custom line items), and no other recorded state prices usage either, so a group's
/// actual charges are zero. Zero is REPORTED as the amount it is rather than withheld or
/// invented, and it is what the pro forma cost — the custom line items the caller really
/// did create — is measured against.
pub const METERED_AWS_COST: f64 = 0.0;

/// How many decimal places a reported amount keeps.
///
/// The reported members are strings carrying a decimal amount, summed and multiplied out
/// of the stored `double` charge values, so the sum carries binary floating-point residue
/// that is not part of any amount. Rounding to this many places drops it while keeping
/// far more precision than the minor unit of either currency this service bills in.
const REPORTED_DECIMALS: i32 = 6;

/// An amount as a reported `AWSCost` / `ProformaCost` / `Margin` / `MarginPercentage`
/// string carries one: a decimal rounded to [`REPORTED_DECIMALS`] places.
pub fn reported_amount(value: f64) -> String {
    let scale = 10f64.powi(REPORTED_DECIMALS);
    // Adding zero normalises a negative zero (a credit cancelling a fee) to the one
    // amount it is, so no row reports `-0`.
    let rounded = (value * scale).round() / scale + 0.0;
    format!("{rounded}")
}

/// The money side of a margin summary, as BOTH of this service's cost reports report it
/// (`BillingGroupCostReportElement` and `BillingGroupCostReportResultElement` carry the
/// same four amounts), so the two can never disagree about one billing group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarginSummary {
    /// "The actual Amazon Web Services charges for the billing group."
    pub aws_cost: f64,
    /// "The hypothetical Amazon Web Services charges based on the associated pricing
    /// plan of a billing group."
    pub proforma_cost: f64,
    /// "The billing group margin."
    pub margin: f64,
    /// "The percentage of billing group margin."
    pub margin_percentage: f64,
}

impl MarginSummary {
    /// The summary of a group whose pro forma charges are `proforma_cost`.
    ///
    /// The margin a billing group earns on a set of charges is what it is charged for
    /// them (pro forma) less what they actually cost (AWS), and the margin percentage is
    /// that margin as a share of the pro forma amount — undefined, and so reported as
    /// zero, where there is no pro forma amount to take a share of.
    pub fn of_proforma(proforma_cost: f64) -> Self {
        let margin = proforma_cost - METERED_AWS_COST;
        let margin_percentage = if proforma_cost == 0.0 {
            0.0
        } else {
            margin / proforma_cost * 100.0
        };
        Self {
            aws_cost: METERED_AWS_COST,
            proforma_cost,
            margin,
            margin_percentage,
        }
    }
}

/// A `PricingPlanArn` as this service stores it. See [`normalize_billing_group_arn`].
///
/// A bare AWS-managed id resolves to the ARN Amazon Web Services owns (account
/// segment `aws`), not to this emulator's account: the `PricingPlanFullArn` pattern
/// allows `(aws|[0-9]{12})` for exactly that reason.
pub fn normalize_pricing_plan_arn(value: &str) -> String {
    if value.starts_with("arn:") {
        value.to_string()
    } else if AWS_MANAGED_PRICING_PLAN_IDS.contains(&value) {
        aws_managed_pricing_plan_arn(value)
    } else {
        pricing_plan_arn(value)
    }
}

/// The ARN of an AWS-managed pricing plan: owned by Amazon Web Services, so its
/// account segment is the literal `aws` the `PricingPlanFullArn` pattern allows
/// beside a 12-digit account.
fn aws_managed_pricing_plan_arn(id: &str) -> String {
    format!("arn:aws:{ARN_SERVICE}:{ARN_REGION}:aws:pricingplan/{id}")
}

/// True when the value is one of the AWS-managed plans of
/// [`AWS_MANAGED_PRICING_PLAN_IDS`] — the WHOLE ARN has to be the one Amazon Web
/// Services owns, so another partition's or another account's ARN ending in the
/// same id is not managed and must exist in state like any other plan.
pub fn is_aws_managed_pricing_plan(arn: &str) -> bool {
    AWS_MANAGED_PRICING_PLAN_IDS
        .iter()
        .any(|id| arn == *id || arn == aws_managed_pricing_plan_arn(id))
}

/// The account every request to this emulator is made by.
///
/// Every generated emulator presents the one canonical account id
/// ([`engine::emulator_account_id`]); it is never derived from the caller's access
/// key. This is the account the authorization gate below judges, and the account id
/// the denial names.
pub fn caller_account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// The modelled `AccessDeniedException` (`@httpError 403`) AWS answers a
/// billing-conductor call made by an account that is not the organization's payer
/// (management) account with.
///
/// The text is built FROM THE REQUEST — the calling account id and the operation name
/// — which is the construction the real service uses; the account id in a recording is
/// environment-specific and an example only. It is deliberately NOT the generic IAM
/// "no identity-based policy allows" wording: that is how a caller tells a per-account
/// entitlement gate from a missing permission.
pub fn not_payer_account(operation: &str) -> crate::ops::types::AccessDeniedException {
    crate::ops::types::AccessDeniedException {
        message: format!(
            "Account {} is not authorized to call {operation}. Only payer account is authorized",
            caller_account_id()
        ),
    }
}

/// The pricing rule type whose rates come from tiers rather than from a
/// percentage modifier. The other values of `PricingRuleType`
/// (`MARKUP`, `DISCOUNT`) price by `ModifierPercentage`.
pub const TIERING_PRICING_RULE_TYPE: &str = "TIERING";

/// The rate members a pricing rule holds: its `ModifierPercentage` and its
/// `Tiering` configuration, of which a rule ever holds exactly one.
pub type PricingRuleRate = (Option<f64>, Option<TieringRecord>);

/// A refusal a shared rule hands back for the calling operation to raise as its
/// OWN modelled `ValidationException` (each operation's error enum is its own
/// type): the reason, the member at fault and the message.
pub type ValidationRefusal = (&'static str, &'static str, String);

/// The member path a refusal naming a tiering configuration's custom tiers carries:
/// the `CustomTiers` member of the request's `Tiering`.
pub const CUSTOM_TIERS_MEMBER: &str = "Tiering.CustomTiers";

/// Does each custom tier price a non-empty usage range, and no two of them the same
/// usage — or is that the refusal the request earns?
///
/// A `CustomTier` carries a half-open usage range ("The inclusive start of the usage
/// range that this tier applies to" / "The exclusive end of the usage range that this
/// tier applies to. If you don't specify a value, this tier applies to all usage that
/// is greater than or equal to `BeginRangeInclusive`") and its rate is the one
/// "applied to the usage that falls within this tier". So a tier whose exclusive end
/// is at or below its inclusive begin prices no usage at all, and two tiers covering
/// one unit of usage leave the rate for that unit undecided: either way the
/// configuration prices usage in a way the service cannot apply, which is what
/// `ValidationExceptionReason` enumerates `ILLEGAL_TIERING_INPUT` for.
///
/// `Err` carries the `(reason, member, message)` of the modelled `ValidationException`
/// the calling operation refuses the request with, exactly as the other shared rules
/// here do, so every path that sets a rule's tiers judges them the same way.
pub fn check_custom_tiers_price_distinct_usage(
    custom_tiers: &[crate::ops::types::CustomTier],
) -> Result<(), ValidationRefusal> {
    for tier in custom_tiers {
        let begin = tier.begin_range_inclusive;
        // Both bounds are numbers at or above zero by the time a request reaches here —
        // the model constrains each of them to `range{min:0}`, which the boundary walk
        // refuses a non-finite or negative value by — so comparing them decides the
        // range.
        //
        // Only a tier that STATES an end can state one covering nothing: an absent end
        // leaves the tier open, so there is no bound to report either.
        if let Some(end) = tier.end_range_exclusive {
            if !half_open_range_is_ordered(Some(begin), Some(end)) {
                return Err((
                    "ILLEGAL_TIERING_INPUT",
                    CUSTOM_TIERS_MEMBER,
                    format!(
                        "the exclusive end of a custom tier's usage range must be greater than \
                         its inclusive begin, but the tier beginning at {begin} ends at {end}"
                    ),
                ));
            }
        }
    }
    // Two of these ranges share usage only if two NEIGHBOURING ones do once they are
    // ordered by their inclusive begin: a range reaching past a later one reaches past
    // every range beginning in between. So one ordering pass decides the whole set, and
    // the work a request's tiers cost stays proportional to the list the request
    // already carried rather than to every pair of it.
    let mut ranges: Vec<(f64, Option<f64>)> = custom_tiers
        .iter()
        .map(|tier| (tier.begin_range_inclusive, tier.end_range_exclusive))
        .collect();
    ranges.sort_by(|left, right| left.0.total_cmp(&right.0));
    for ((earlier_begin, earlier_end), (later_begin, later_end)) in
        ranges.iter().zip(ranges.iter().skip(1))
    {
        if half_open_ranges_overlap(
            (Some(*earlier_begin), *earlier_end),
            (Some(*later_begin), *later_end),
        ) {
            return Err((
                "ILLEGAL_TIERING_INPUT",
                CUSTOM_TIERS_MEMBER,
                format!(
                    "no two custom tiers may price the same usage, but the tiers beginning at \
                     {earlier_begin} and {later_begin} cover common usage"
                ),
            ));
        }
    }
    Ok(())
}

/// The rate members a pricing rule of `rule_type` must hold, or the refusal the
/// request earns.
///
/// A rule prices EITHER by a percentage (`MARKUP`/`DISCOUNT`) or by tiers
/// (`TIERING`), never both, which is what the reasons `ValidationExceptionReason`
/// enumerates for each of them (`ILLEGAL_MODIFIER_PERCENTAGE`,
/// `ILLEGAL_TIERING_INPUT`) refuse a rule for. So a request that NAMES the member
/// its resulting type does not price by is refused, the one the type needs has to
/// be there (named by the request or already stored), and a value only carried
/// over from storage into a new type is dropped rather than refused.
///
/// `Ok` carries the `(ModifierPercentage, Tiering)` pair to store; `Err` carries
/// the `(reason, member, message)` of the modelled `ValidationException` the
/// calling operation refuses the request with. The tiers a `TIERING` rule prices by
/// are judged here too, by [`check_custom_tiers_price_distinct_usage`], so one rule
/// is never stored pricing usage the service cannot apply. BOTH paths that set a
/// rule's rate decide it here, so one rule has one behaviour: `UpdatePricingRule`
/// passes the stored record merged with the request, and `CreatePricingRule` — which
/// has no stored record behind it — passes the request's own members as the whole
/// rate.
pub fn reconcile_pricing_rule_rate(
    rule_type: &str,
    modifier_percentage: Option<f64>,
    tiering: Option<TieringRecord>,
    modifier_named_by_request: bool,
    tiering_named_by_request: bool,
) -> Result<PricingRuleRate, ValidationRefusal> {
    if rule_type == TIERING_PRICING_RULE_TYPE {
        if modifier_named_by_request {
            return Err((
                "ILLEGAL_MODIFIER_PERCENTAGE",
                "ModifierPercentage",
                format!("a {TIERING_PRICING_RULE_TYPE} pricing rule has no modifier percentage"),
            ));
        }
        let tiering = tiering.filter(|tiering| !tiering.is_empty());
        if tiering.is_none() {
            return Err((
                "ILLEGAL_TIERING_INPUT",
                "Tiering",
                format!("a {TIERING_PRICING_RULE_TYPE} pricing rule needs a tiering configuration"),
            ));
        }
        // The tiers the rule will price by have to describe usage the service can
        // actually price — the shared rule above, applied on every path that sets a
        // rule's tiers.
        if let Some(custom_tiers) = tiering
            .as_ref()
            .and_then(|tiering| tiering.custom_tiers.as_deref())
        {
            check_custom_tiers_price_distinct_usage(custom_tiers)?;
        }
        return Ok((None, tiering));
    }
    if tiering_named_by_request {
        return Err((
            "ILLEGAL_TIERING_INPUT",
            "Tiering",
            format!("a {rule_type} pricing rule has no tiering configuration"),
        ));
    }
    if modifier_percentage.is_none() {
        return Err((
            "ILLEGAL_MODIFIER_PERCENTAGE",
            "ModifierPercentage",
            format!("a {rule_type} pricing rule needs a modifier percentage"),
        ));
    }
    Ok((modifier_percentage, None))
}

/// Whether the members a pricing rule's `Scope` keys on — and ONLY those — are the
/// ones the request names, or the refusal the request earns.
///
/// A pricing rule describes exactly ONE scope, and the members outside that scope's
/// key are not part of the rule: the model's own member documentation conditions
/// each of them on the scope (`Service`: "If the Scope attribute is set to SERVICE or
/// SKU, the attribute indicates which service the PricingRule is applicable for";
/// `UsageType` / `Operation`: "If the Scope attribute is set to SKU, this attribute
/// indicates which usage type / operation the PricingRule is modifying";
/// `BillingEntity` is the seller a `BILLING_ENTITY` rule prices). So the condition
/// holds in BOTH directions — a scope-keyed member has to be there, and a member the
/// scope does not key on may not be — because a rule holding either way round
/// describes pricing the service cannot apply. Each refusal carries the reason
/// `ValidationExceptionReason` enumerates for that member (`ILLEGAL_SERVICE`,
/// `ILLEGAL_USAGE_TYPE`, `ILLEGAL_OPERATION`, `ILLEGAL_BILLING_ENTITY`).
///
/// `Err` carries the `(reason, member, message)` of the modelled
/// `ValidationException` the calling operation refuses the request with, exactly as
/// [`reconcile_pricing_rule_rate`] hands back the rate refusals, so every path that
/// sets a rule's scope judges it the same way.
pub fn check_pricing_rule_scope_members(
    scope: &str,
    service: Option<&str>,
    usage_type: Option<&str>,
    operation: Option<&str>,
    billing_entity: Option<&str>,
) -> Result<(), ValidationRefusal> {
    // Per member: its name, how a message names it, the reason enumerated for it, the
    // scopes it keys on, and whether this request named it. The member and the value
    // sit in one row so the two cannot drift apart.
    let members: [(&'static str, &'static str, &'static str, &[&str], bool); 4] = [
        (
            "Service",
            "a Service",
            "ILLEGAL_SERVICE",
            &["SERVICE", "SKU"],
            service.is_some(),
        ),
        (
            "UsageType",
            "a UsageType",
            "ILLEGAL_USAGE_TYPE",
            &["SKU"],
            usage_type.is_some(),
        ),
        (
            "Operation",
            "an Operation",
            "ILLEGAL_OPERATION",
            &["SKU"],
            operation.is_some(),
        ),
        (
            "BillingEntity",
            "a BillingEntity",
            "ILLEGAL_BILLING_ENTITY",
            &["BILLING_ENTITY"],
            billing_entity.is_some(),
        ),
    ];
    for (member, phrase, reason, keyed_scopes, named) in members {
        match (keyed_scopes.contains(&scope), named) {
            (true, false) => {
                return Err((
                    reason,
                    member,
                    format!("a pricing rule scoped to {scope} must name {phrase}"),
                ));
            }
            (false, true) => {
                return Err((
                    reason,
                    member,
                    format!("a pricing rule scoped to {scope} does not take {phrase}"),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// One GENERATION of a billing-period-versioned resource: the members a write can
/// change, frozen as they stood while that generation was in force.
///
/// The one thing every generation has to answer is WHEN it took effect: `LastModifiedTime`
/// is the instant of the write that left it, and the billing period that instant falls in
/// is the period the generation is in effect FROM ([`Self::start_billing_period`]). So
/// that derivation is written here once and every versioned resource's generation
/// ([`PricingPlanVersion`], [`PricingRuleVersion`], [`BillingGroupVersion`]) answers it
/// the same way.
pub trait ResourceGeneration: Clone {
    /// `LastModifiedTime` as this generation reported it — the instant of the write that
    /// left it.
    fn last_modified_time_millis(&self) -> i64;

    /// The billing period this generation is in effect FROM, inclusive: the period the
    /// write that left it was made in, derived from that write's instant the same way
    /// [`VersionedResource::version_start_billing_period`] derives the live one.
    fn start_billing_period(&self) -> String {
        billing_period_at(self.last_modified_time_millis())
    }
}

/// A record every read of which is scoped to a BILLING PERIOD: the resource as it stands
/// now, plus the generations of it that later writes superseded.
///
/// A pricing plan, a pricing rule and a billing group are all versioned this way, and
/// they answer ONE rule — the generation in force in a period is the latest one whose own
/// start period is at or before it, and a resource exists in no period before the one it
/// was created in — so the rule lives here, written once. A record only says what a
/// generation of it IS ([`Self::current_version`]), where its history lives
/// ([`Self::superseded_versions`]) and how much of that history is kept
/// ([`Self::MAX_SUPERSEDED_VERSIONS`]); the write side of the same rule is
/// [`put_versioned`].
pub trait VersionedResource {
    /// One generation of this resource — the members a write can change.
    type Generation: ResourceGeneration;

    /// How many SUPERSEDED generations of one such resource the store keeps; past it the
    /// OLDEST is dropped ([`put_versioned`]), which reads back as the oldest kept
    /// generation answering for the periods before it.
    const MAX_SUPERSEDED_VERSIONS: usize;

    /// The FULL ARN this record is keyed by.
    fn arn(&self) -> &str;

    /// `CreationTime`: the instant the call that minted this resource was made at.
    fn creation_time_millis(&self) -> i64;

    /// `LastModifiedTime`: the instant of the write that left the resource as it stands.
    fn last_modified_time_millis(&self) -> i64;

    /// This resource's CURRENT generation, as a snapshot of the members a write can
    /// change.
    ///
    /// Projected from the live record rather than stored twice, so the newest generation
    /// is the resource a read of the current period reports.
    fn current_version(&self) -> Self::Generation;

    /// The generations of this resource that later writes SUPERSEDED, oldest first. The
    /// generation in effect NOW is not among them — it is projected by
    /// [`Self::current_version`] — so the history and the live resource cannot drift.
    fn superseded_versions(&self) -> &[Self::Generation];

    /// The same kept history, for the one writer that appends to it
    /// ([`put_versioned`]).
    fn superseded_versions_mut(&mut self) -> &mut Vec<Self::Generation>;

    /// The billing period this resource was created in: the first period any read of it
    /// reports it for, since a resource does not exist in a period that precedes it.
    fn creation_billing_period(&self) -> String {
        billing_period_at(self.creation_time_millis())
    }

    /// The billing period this resource's CURRENT generation is in effect FROM.
    ///
    /// A write takes effect in the period it is made in, and `LastModifiedTime` is the
    /// instant of the write that left the resource as it stands, so the period that
    /// instant falls in is the one the current generation starts in. A second write in
    /// the same period leaves that period unchanged, which is why one period has one
    /// generation.
    fn version_start_billing_period(&self) -> String {
        billing_period_at(self.last_modified_time_millis())
    }

    /// Every generation of this resource, OLDEST FIRST: the ones later writes superseded,
    /// frozen as they stood, and then the one in effect now.
    fn versions(&self) -> Vec<Self::Generation> {
        let mut versions = self.superseded_versions().to_vec();
        versions.push(self.current_version());
        versions
    }

    /// Does this resource exist in `billing_period`? A resource does not exist in a
    /// period that precedes the one it was created in, and a value that is not a billing
    /// period at all names no period a resource exists in.
    ///
    /// The cheap half of [`Self::effective_in`], so a listing can cut its page before
    /// projecting the generations the page reports.
    fn exists_in(&self, billing_period: &str) -> bool {
        match (
            billing_period_key(billing_period),
            billing_period_key(&self.creation_billing_period()),
        ) {
            (Some(wanted), Some(created)) => wanted >= created,
            _ => false,
        }
    }

    /// The generation of this resource that is in force in `billing_period` — what a read
    /// scoped to that period reports — or `None` when the resource did not exist then
    /// ([`Self::exists_in`]).
    ///
    /// The generation in force is the LATEST one whose own start period is at or before
    /// the period asked about, so a write made later never shows up in an earlier period.
    /// When the oldest generations have been dropped from the kept history
    /// ([`Self::MAX_SUPERSEDED_VERSIONS`]), the oldest one still kept answers for the
    /// periods before it, because the resource itself did exist then.
    fn effective_in(&self, billing_period: &str) -> Option<Self::Generation> {
        if !self.exists_in(billing_period) {
            return None;
        }
        let wanted = billing_period_key(billing_period)?;
        let versions = self.versions();
        versions
            .iter()
            .rev()
            .find(|version| {
                billing_period_key(&version.start_billing_period())
                    .is_some_and(|start| start <= wanted)
            })
            .or_else(|| versions.first())
            .cloned()
    }
}

/// Store `updated` as `store`'s record for its ARN, keeping the generation it replaces
/// when the write takes effect in a DIFFERENT billing period.
///
/// A generation of a versioned resource is the resource as it stood while it was in
/// force, so the billing period a write takes effect in decides what the write does to
/// the history: a write landing in a period AFTER the one the current generation started
/// in closes that generation — frozen as it stood, so a later read of that period reports
/// the resource as it really was — while a write landing in the generation's own period
/// REPLACES it, because one billing period has one generation of the resource.
///
/// The kept history is bounded like every other store here: past
/// [`VersionedResource::MAX_SUPERSEDED_VERSIONS`] superseded generations the OLDEST is
/// dropped, which reads back as the oldest kept generation covering the periods before it
/// — never as a resource that vanishes from a period it existed in, which a later caller
/// must still see ([`VersionedResource::effective_in`]).
///
/// One implementation for every versioned resource, so a correction to the rule reaches
/// the pricing plans, the pricing rules and the billing groups at once.
pub fn put_versioned<Record: VersionedResource>(
    store: &mut BTreeMap<String, Record>,
    mut updated: Record,
) {
    let arn = updated.arn().to_string();
    let superseded = store
        .get(&arn)
        .filter(|previous| {
            previous.version_start_billing_period() != updated.version_start_billing_period()
        })
        .map(Record::current_version);
    if let Some(superseded) = superseded {
        let kept = updated.superseded_versions_mut();
        kept.push(superseded);
        while kept.len() > Record::MAX_SUPERSEDED_VERSIONS {
            kept.remove(0);
        }
    }
    store.insert(arn, updated);
}

/// A pricing plan as `CreatePricingPlan` persisted it.
///
/// The record carries the plan as it stands NOW, plus the generations of it that later
/// writes superseded ([`PricingPlanVersion`]), because every read of a plan is scoped to
/// a billing period and so reports the generation that was in force in that period —
/// the same shape [`PricingRuleRecord`] and [`BillingGroupRecord`] carry. The rule that
/// decides which generation a period reports, and the write rule that opens one, are
/// [`VersionedResource`] and [`put_versioned`], implemented once for all three.
#[derive(Debug, Clone, Default)]
pub struct PricingPlanRecord {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// The rules this plan associates, each with the billing periods its association
    /// is in force over — including the ones that have ENDED, which a read scoped to a
    /// period they were in force in still reports; see
    /// [`PricingRuleAssociationRecord`].
    pub pricing_rule_associations: Vec<PricingRuleAssociationRecord>,
    pub tags: BTreeMap<String, String>,
    pub creation_time_millis: i64,
    pub last_modified_time_millis: i64,
    /// The generations of this plan that later writes SUPERSEDED, oldest first, at most
    /// [`MAX_PRICING_PLAN_VERSIONS`] of them. The generation in effect NOW is not among
    /// them — it is projected from this record by [`Self::current_version`] — so the
    /// history and the live plan cannot drift. Held on the record so a deleted plan takes
    /// its history with it.
    pub superseded_versions: Vec<PricingPlanVersion>,
}

impl PricingPlanRecord {
    /// The FULL ARNs of the rules this plan associates IN `billing_period`,
    /// deduplicated and in ARN order.
    ///
    /// The association is to the RULE, not to the spelling it was recorded with, so
    /// two spellings of one rule collapse to one association.
    pub fn associated_rule_arns(&self, billing_period: &str) -> Vec<String> {
        self.pricing_rule_associations
            .iter()
            .filter(|association| association.covers(billing_period))
            .map(|association| normalize_pricing_rule_arn(&association.arn))
            .collect::<BTreeSet<String>>()
            .into_iter()
            .collect()
    }

    /// End this plan's association with each rule of `rule_arns` AT `billing_period`,
    /// the period the disassociating call is made in: the association is in force in
    /// every period up to that one and in none from it on.
    ///
    /// `rule_arns` are FULL rule ARNs ([`normalize_pricing_rule_arn`]), and only the
    /// associations that still stand are ended, so a rule named twice over two calls in
    /// one period does not reopen an association that has already ended.
    ///
    /// An association ended in the very period it started in was in force in no period
    /// at all ([`PricingRuleAssociationRecord::covers`]), so its record is DROPPED
    /// rather than kept as a span covering nothing — which is also what keeps a caller
    /// cycling one rule inside one billing period from growing the record. The ended
    /// associations that a read of a closed period still has to report are kept, at most
    /// [`MAX_ENDED_PRICING_RULE_ASSOCIATIONS_PER_PRICING_PLAN`] of them with the oldest
    /// dropped first, so the kept history is bounded like every other store here and the
    /// associations in force are never the ones evicted.
    pub fn end_rule_associations(&mut self, rule_arns: &[String], billing_period: &str) {
        for association in &mut self.pricing_rule_associations {
            if association.is_live() && rule_arns.contains(&association.arn) {
                association.exclusive_end_billing_period = Some(billing_period.to_string());
            }
        }
        self.pricing_rule_associations
            .retain(PricingRuleAssociationRecord::covers_any_period);
        let cap = MAX_ENDED_PRICING_RULE_ASSOCIATIONS_PER_PRICING_PLAN;
        while self.ended_rule_association_count() > cap {
            let Some(oldest) = self
                .pricing_rule_associations
                .iter()
                .position(|association| !association.is_live())
            else {
                break;
            };
            self.pricing_rule_associations.remove(oldest);
        }
    }

    /// How many of this plan's associations have ENDED — the history the cap
    /// [`MAX_ENDED_PRICING_RULE_ASSOCIATIONS_PER_PRICING_PLAN`] bounds.
    fn ended_rule_association_count(&self) -> usize {
        self.pricing_rule_associations
            .iter()
            .filter(|association| !association.is_live())
            .count()
    }

    /// Do this record and `other` hold the SAME values for every member a write can
    /// change — exactly the members one generation of the plan carries
    /// ([`VersionedResource::current_version`]), named here once so the two cannot drift?
    ///
    /// What decides whether a write modified the plan at all: `LastModifiedTime` is the
    /// instant of the write that left the plan as it stands, and the generation in force
    /// starts in the period that instant falls in, so a write leaving every one of these
    /// members where it was neither moves the instant nor opens a generation.
    pub fn holds_same_members_as(&self, other: &Self) -> bool {
        self.name == other.name && self.description == other.description
    }
}

/// The billing-period versioning rule, as a pricing plan carries it: a generation of a
/// plan is the `Name` and `Description` a write can change, and at most
/// [`MAX_PRICING_PLAN_VERSIONS`] superseded ones are kept. Everything a read of a period
/// does with them is [`VersionedResource`]'s.
impl VersionedResource for PricingPlanRecord {
    type Generation = PricingPlanVersion;

    const MAX_SUPERSEDED_VERSIONS: usize = MAX_PRICING_PLAN_VERSIONS;

    fn arn(&self) -> &str {
        &self.arn
    }

    fn creation_time_millis(&self) -> i64 {
        self.creation_time_millis
    }

    fn last_modified_time_millis(&self) -> i64 {
        self.last_modified_time_millis
    }

    fn current_version(&self) -> PricingPlanVersion {
        PricingPlanVersion {
            name: self.name.clone(),
            description: self.description.clone(),
            last_modified_time_millis: self.last_modified_time_millis,
        }
    }

    fn superseded_versions(&self) -> &[PricingPlanVersion] {
        &self.superseded_versions
    }

    fn superseded_versions_mut(&mut self) -> &mut Vec<PricingPlanVersion> {
        &mut self.superseded_versions
    }
}

/// One GENERATION of a pricing plan: the members a write can change, as they stood while
/// that generation was in force.
///
/// A pricing plan read is scoped to a billing period ("The billing period for which the
/// described pricing plans are applicable"), so the values a read reports are the ones
/// that held in that period. A superseded generation is FROZEN: nothing writes to it
/// again, so a read of a closed period reports the plan as it really was rather than as
/// it has since become. The members no write can change — the ARN and `CreationTime`,
/// neither of which `UpdatePricingPlanInput` carries — stay on the record, so there is
/// one copy of them, and `Size` is derived per period from the plan's associations
/// ([`PricingPlanRecord::associated_rule_arns`]) rather than frozen into a generation.
#[derive(Debug, Clone, Default)]
pub struct PricingPlanVersion {
    pub name: String,
    pub description: Option<String>,
    /// `LastModifiedTime` as this generation reported it, which is also the instant the
    /// generation took effect — see
    /// [`ResourceGeneration::start_billing_period`].
    pub last_modified_time_millis: i64,
}

impl ResourceGeneration for PricingPlanVersion {
    fn last_modified_time_millis(&self) -> i64 {
        self.last_modified_time_millis
    }
}

/// One pricing rule association on a pricing plan, with the billing periods it is in
/// force over.
///
/// A Billing Conductor association is scoped to a billing period — every read of one
/// names the period it is asking about (`BillingPeriod`: "the billing period for which
/// the pricing rule associations are to be listed") — so the record keeps the periods
/// the association is in force over rather than only the rule. The span is half-open,
/// exactly as every other billing-period range here is (see
/// [`billing_period_range_covers`]): the association holds from
/// `inclusive_start_billing_period`, the period the associating call was made in, and
/// `exclusive_end_billing_period` is the first period it no longer holds — absent while
/// it still stands, the same shape a billing group membership
/// ([`AccountAssociationRecord`]) carries.
/// `Default` is deliberately NOT derived: an empty start period is not a billing
/// period, and a record that carried one would be read as unbounded. Every
/// association is built through [`Self::started_now`].
#[derive(Debug, Clone)]
pub struct PricingRuleAssociationRecord {
    /// The FULL pricing rule ARN ([`normalize_pricing_rule_arn`]).
    pub arn: String,
    /// The first billing period this association is in force for.
    pub inclusive_start_billing_period: String,
    /// The first billing period it is no longer in force for, absent while it stands.
    pub exclusive_end_billing_period: Option<String>,
}

impl PricingRuleAssociationRecord {
    /// An association made NOW: in force from the current billing period onward.
    pub fn started_now(rule_arn: &str) -> Self {
        Self {
            arn: normalize_pricing_rule_arn(rule_arn),
            inclusive_start_billing_period: current_billing_period(),
            exclusive_end_billing_period: None,
        }
    }

    /// Does this association still stand — has nothing ended it?
    pub fn is_live(&self) -> bool {
        self.exclusive_end_billing_period.is_none()
    }

    /// Is this association in force in ANY billing period at all? One that still
    /// stands always is; an ended one only when it ended in a LATER period than the one
    /// it started in, since the span is half-open.
    pub fn covers_any_period(&self) -> bool {
        billing_period_precedes(
            billing_period_key(&self.inclusive_start_billing_period),
            self.exclusive_end_billing_period
                .as_deref()
                .and_then(billing_period_key),
        )
    }

    /// Is this association in force in `billing_period`? Half-open, as
    /// [`billing_period_range_covers`] reads it: every period from the one it started
    /// in up to, but not including, the one it ended at — so an association that began
    /// and ended in the SAME period is in force in no period at all.
    pub fn covers(&self, billing_period: &str) -> bool {
        billing_period_range_covers(
            Some(&self.inclusive_start_billing_period),
            self.exclusive_end_billing_period.as_deref(),
            billing_period,
        )
    }
}

/// A pricing rule as `CreatePricingRule` persisted it.
///
/// `AssociatedPricingPlanCount` is DERIVED from the pricing plans that reference
/// the rule (see
/// [`associated_pricing_plan_count`](ComAmazonawsBillingconductorAwsbillingconductorState::associated_pricing_plan_count)),
/// never stored, so it cannot drift from the associations it counts.
///
/// The record carries the rule as it stands NOW, plus the generations of it that later
/// writes superseded ([`PricingRuleVersion`]), because every read of a rule is scoped to
/// a billing period and so reports the generation that was in force in that period.
#[derive(Debug, Clone, Default)]
pub struct PricingRuleRecord {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// `PricingRuleScope`, one of `PRICING_RULE_SCOPE_VALUES`.
    pub scope: String,
    /// `PricingRuleType`, one of `PRICING_RULE_TYPE_VALUES`.
    pub rule_type: String,
    pub modifier_percentage: Option<f64>,
    pub service: Option<String>,
    pub billing_entity: Option<String>,
    pub tiering: Option<TieringRecord>,
    pub usage_type: Option<String>,
    pub operation: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub creation_time_millis: i64,
    pub last_modified_time_millis: i64,
    /// The generations of this rule that later writes SUPERSEDED, oldest first, at
    /// most [`MAX_PRICING_RULE_VERSIONS`] of them. The generation in effect NOW is not
    /// among them — it is projected from this record by [`Self::current_version`] — so
    /// the history and the live rule cannot drift. Held on the record so a deleted rule
    /// takes its history with it.
    pub superseded_versions: Vec<PricingRuleVersion>,
}

impl PricingRuleRecord {
    /// Do this record and `other` hold the SAME values for every member a write can
    /// change — exactly the members one generation of the rule carries
    /// ([`VersionedResource::current_version`]), named here once so the two cannot drift?
    ///
    /// What decides whether a write modified the rule at all: `LastModifiedTime` is the
    /// instant of the write that left the rule as it stands, and the generation in force
    /// starts in the period that instant falls in, so a write leaving every one of these
    /// members where it was neither moves the instant nor opens a generation.
    pub fn holds_same_members_as(&self, other: &Self) -> bool {
        self.name == other.name
            && self.description == other.description
            && self.rule_type == other.rule_type
            && self.modifier_percentage == other.modifier_percentage
            && self.tiering == other.tiering
    }
}

/// The billing-period versioning rule, as a pricing rule carries it: a generation of a
/// rule is the `Name`, `Description`, `Type`, `ModifierPercentage` and `Tiering` a write
/// can change, and at most [`MAX_PRICING_RULE_VERSIONS`] superseded ones are kept.
/// Everything a read of a period does with them is [`VersionedResource`]'s.
impl VersionedResource for PricingRuleRecord {
    type Generation = PricingRuleVersion;

    const MAX_SUPERSEDED_VERSIONS: usize = MAX_PRICING_RULE_VERSIONS;

    fn arn(&self) -> &str {
        &self.arn
    }

    fn creation_time_millis(&self) -> i64 {
        self.creation_time_millis
    }

    fn last_modified_time_millis(&self) -> i64 {
        self.last_modified_time_millis
    }

    fn current_version(&self) -> PricingRuleVersion {
        PricingRuleVersion {
            name: self.name.clone(),
            description: self.description.clone(),
            rule_type: self.rule_type.clone(),
            modifier_percentage: self.modifier_percentage,
            tiering: self.tiering.clone(),
            last_modified_time_millis: self.last_modified_time_millis,
        }
    }

    fn superseded_versions(&self) -> &[PricingRuleVersion] {
        &self.superseded_versions
    }

    fn superseded_versions_mut(&mut self) -> &mut Vec<PricingRuleVersion> {
        &mut self.superseded_versions
    }
}

/// One GENERATION of a pricing rule: the members a write can change, as they stood
/// while that generation was in force.
///
/// A pricing rule read is scoped to a billing period ("The billing period for which the
/// described pricing rules are applicable"), so the values a read reports are the ones
/// that held in that period. A superseded generation is FROZEN: nothing writes to it
/// again, so a read of a closed period reports the rule as it really was rather than as
/// it has since become. The members no write can change — the ARN, `Scope`, `Service`,
/// `BillingEntity`, `UsageType`, `Operation` and `CreationTime`, none of which
/// `UpdatePricingRuleInput` carries — stay on the record, so there is one copy of them.
#[derive(Debug, Clone, Default)]
pub struct PricingRuleVersion {
    pub name: String,
    pub description: Option<String>,
    /// `PricingRuleType`, one of `PRICING_RULE_TYPE_VALUES`.
    pub rule_type: String,
    pub modifier_percentage: Option<f64>,
    pub tiering: Option<TieringRecord>,
    /// `LastModifiedTime` as this generation reported it, which is also the instant the
    /// generation took effect — see
    /// [`ResourceGeneration::start_billing_period`].
    pub last_modified_time_millis: i64,
}

impl ResourceGeneration for PricingRuleVersion {
    fn last_modified_time_millis(&self) -> i64 {
        self.last_modified_time_millis
    }
}

/// A pricing rule's tiering configuration.
///
/// `CreateTieringInput` and the `Tiering` a read reports differ only in their
/// free-tier structure (`CreateFreeTierConfig` / `FreeTierConfig`, both a single
/// `Activated` boolean), and both carry the SAME `CustomTier` shape — so the
/// boolean is what is stored and `CustomTier` is reused as modelled.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TieringRecord {
    /// `Tiering.FreeTier.Activated`, absent when the request named no free tier.
    pub free_tier_activated: Option<bool>,
    /// `Tiering.CustomTiers`, in the order the request listed them.
    pub custom_tiers: Option<Vec<crate::ops::types::CustomTier>>,
}

impl TieringRecord {
    /// True when the record holds no tiering configuration, which is how an
    /// update that names neither member leaves a rule with no `Tiering` at all.
    pub fn is_empty(&self) -> bool {
        self.free_tier_activated.is_none()
            && self.custom_tiers.as_ref().is_none_or(Vec::is_empty)
    }
}

/// The billing group statuses a CALLER may ask for, out of the three the modelled
/// `BillingGroupStatus` enumerates
/// (`smithy_query(query="shape",shape="BillingGroupStatus")`).
///
/// `PRIMARY_ACCOUNT_MISSING` is not among them: it reports that the group has no
/// billing owner, a condition of the resource the service DERIVES (see
/// [`BillingGroupRecord::status`]), so it is not a status a request chooses. The
/// other two say whether the group is billing (`ACTIVE`) or not yet (`PENDING`),
/// which is the group's own operational state and so is the caller's to set.
pub const CALLER_SETTABLE_BILLING_GROUP_STATUSES: &[&str] = &["ACTIVE", "PENDING"];

/// A billing group as `CreateBillingGroup` persisted it. `BillingGroupType` and the owner
/// half of `Status` are DERIVED (see the methods below), never stored, so they cannot
/// drift from the preferences they are computed from. `Size` is derived too, but not from
/// this record: the membership a billing period reports is kept per period on the account
/// memberships, so the one rule that counts it is
/// [`ComAmazonawsBillingconductorAwsbillingconductorState::billing_group_sizes_in`] and
/// every reader of a group reports the count it answers with.
///
/// The record carries the group as it stands NOW, plus the generations of it that later
/// writes superseded ([`BillingGroupVersion`]), because every read of a billing group is
/// scoped to a billing period and so reports the generation that was in force in that
/// period — the same shape [`PricingRuleRecord`] keeps for a pricing rule.
#[derive(Debug, Clone, Default)]
pub struct BillingGroupRecord {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub primary_account_id: Option<String>,
    pub pricing_plan_arn: String,
    pub linked_account_ids: Vec<String>,
    pub auto_associate: Option<bool>,
    pub responsibility_transfer_arn: Option<String>,
    /// The operational `BillingGroupStatus` a caller asked this group to hold, one
    /// of [`CALLER_SETTABLE_BILLING_GROUP_STATUSES`]. `None` for a group no caller
    /// has set a status on, which reads back as `ACTIVE` — the status a group with a
    /// billing owner holds until something says otherwise.
    pub requested_status: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub creation_time_millis: i64,
    pub last_modified_time_millis: i64,
    /// The generations of this group that later writes SUPERSEDED, oldest first, at most
    /// [`MAX_BILLING_GROUP_VERSIONS`] of them. The generation in effect NOW is not among
    /// them — it is projected from this record by [`Self::current_version`] — so the
    /// history and the live group cannot drift. Held on the record so a deleted group
    /// takes its history with it.
    pub superseded_versions: Vec<BillingGroupVersion>,
}

impl BillingGroupRecord {
    /// `BillingGroupStatus` as the generation `version` reported it — the status a read
    /// scoped to that generation's periods answers with.
    ///
    /// A group with no billing owner ([`has_billing_owner`](Self::has_billing_owner)) is
    /// `PRIMARY_ACCOUNT_MISSING`, whatever any request asked for: that status reports the
    /// absence rather than a choice, and neither member it is decided from is one a write
    /// can change, so it belongs to the record rather than to a generation.
    pub fn status_of(&self, version: &BillingGroupVersion) -> &'static str {
        if !self.has_billing_owner() {
            return "PRIMARY_ACCOUNT_MISSING";
        }
        match version.requested_status.as_deref() {
            Some("PENDING") => "PENDING",
            _ => "ACTIVE",
        }
    }

    /// `StatusReason` for the generation `version`: why the group was in
    /// [`status_of`](Self::status_of) then, absent when that status is `ACTIVE`.
    pub fn status_reason_of(&self, version: &BillingGroupVersion) -> Option<String> {
        match self.status_of(version) {
            "PRIMARY_ACCOUNT_MISSING" => Some(
                "The billing group has no primary account and no responsibility transfer ARN."
                    .to_string(),
            ),
            "PENDING" => {
                Some("The billing group is not billing yet at its owner's request.".to_string())
            }
            _ => None,
        }
    }

    /// True when the group has a billing owner: either a primary account, or a
    /// responsibility transfer relationship — the model documents
    /// `AccountGrouping.ResponsibilityTransferArn` as "When specified, the
    /// PrimaryAccountId is no longer required".
    pub fn has_billing_owner(&self) -> bool {
        self.primary_account_id.is_some() || self.responsibility_transfer_arn.is_some()
    }

    /// `BillingGroupStatus` as the group stands NOW: the status of its current
    /// generation ([`status_of`](Self::status_of)), which is what a write answers with
    /// and what a read of the current billing period reports.
    pub fn status(&self) -> &'static str {
        self.status_of(&self.current_version())
    }

    /// `StatusReason`: why the group is in [`status`](Self::status), absent when it
    /// is `ACTIVE`.
    pub fn status_reason(&self) -> Option<String> {
        self.status_reason_of(&self.current_version())
    }

    /// True when this group automatically associates the accounts newly joining the
    /// consolidated billing family — `AccountGrouping.AutoAssociate`, where an absent
    /// flag is the default `false`. One reading of the flag, so every rule that turns
    /// on it (the state-wide cap on holders, the refusal to delete a holder) decides
    /// from the same answer.
    pub fn auto_associates(&self) -> bool {
        self.auto_associate.unwrap_or(false)
    }

    /// `BillingGroupType`: a group that bills through a transfer relationship is
    /// `TRANSFER_BILLING`, every other group is `STANDARD`.
    pub fn billing_group_type(&self) -> &'static str {
        if self.responsibility_transfer_arn.is_some() {
            "TRANSFER_BILLING"
        } else {
            "STANDARD"
        }
    }
}

/// The billing-period versioning rule, as a billing group carries it: a generation of a
/// group is the `Name`, `Description`, `ComputationPreference.PricingPlanArn`,
/// `AccountGrouping.AutoAssociate` and requested status a write can change, and at most
/// [`MAX_BILLING_GROUP_VERSIONS`] superseded ones are kept. Everything a read of a period
/// does with them is [`VersionedResource`]'s.
impl VersionedResource for BillingGroupRecord {
    type Generation = BillingGroupVersion;

    const MAX_SUPERSEDED_VERSIONS: usize = MAX_BILLING_GROUP_VERSIONS;

    fn arn(&self) -> &str {
        &self.arn
    }

    fn creation_time_millis(&self) -> i64 {
        self.creation_time_millis
    }

    fn last_modified_time_millis(&self) -> i64 {
        self.last_modified_time_millis
    }

    fn current_version(&self) -> BillingGroupVersion {
        BillingGroupVersion {
            name: self.name.clone(),
            description: self.description.clone(),
            pricing_plan_arn: self.pricing_plan_arn.clone(),
            auto_associate: self.auto_associate,
            requested_status: self.requested_status.clone(),
            last_modified_time_millis: self.last_modified_time_millis,
        }
    }

    fn superseded_versions(&self) -> &[BillingGroupVersion] {
        &self.superseded_versions
    }

    fn superseded_versions_mut(&mut self) -> &mut Vec<BillingGroupVersion> {
        &mut self.superseded_versions
    }
}

/// One GENERATION of a billing group: the members a write can change, as they stood
/// while that generation was in force.
///
/// A billing group read is scoped to a billing period (`ListBillingGroups`: "retrieve a
/// list of billing groups for the given billing period"), so the values a read reports
/// are the ones that held in that period. A superseded generation is FROZEN: nothing
/// writes to it again, so a read of a closed period reports the group as it really was
/// rather than as it has since become. The members no write can change — the ARN, the
/// primary account, the responsibility transfer ARN and `CreationTime`, none of which
/// `UpdateBillingGroupInput` can change
/// (`smithy_query(query="shape",shape="UpdateBillingGroupInput")`) — stay on the record,
/// so there is one copy of them. The membership a period reports is not here either: it
/// is kept per period on the account memberships themselves
/// ([`ComAmazonawsBillingconductorAwsbillingconductorState::billing_group_sizes_in`]).
#[derive(Debug, Clone, Default)]
pub struct BillingGroupVersion {
    pub name: String,
    pub description: Option<String>,
    /// `ComputationPreference.PricingPlanArn` as this generation carried it.
    pub pricing_plan_arn: String,
    /// `AccountGrouping.AutoAssociate` as this generation carried it.
    pub auto_associate: Option<bool>,
    /// The operational status a caller had asked for while this generation was in force —
    /// see [`BillingGroupRecord::requested_status`].
    pub requested_status: Option<String>,
    /// `LastModifiedTime` as this generation reported it, which is also the instant the
    /// generation took effect — see
    /// [`ResourceGeneration::start_billing_period`].
    pub last_modified_time_millis: i64,
}

impl ResourceGeneration for BillingGroupVersion {
    fn last_modified_time_millis(&self) -> i64 {
        self.last_modified_time_millis
    }
}

/// A billing group that was DELETED, with the instant the delete was made at: the
/// record of the billing periods that group billed in, which the delete does not
/// erase.
///
/// A billing group bills a consolidated family FOR A BILLING PERIOD, so a period it
/// billed in keeps a record of it after it is gone — which is what the model's
/// `BILLING_GROUP_ALREADY_EXIST_IN_CURRENT_BILLING_PERIOD` refusal is decided from
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`): a name a group
/// occupied in a billing period stays occupied for that period. The whole record is
/// kept rather than just its name, because the name a period reports is the one the
/// generation in force THEN carried ([`Self::name_in`]).
#[derive(Debug, Clone)]
pub struct DeletedBillingGroupRecord {
    /// The group as it stood when the delete took it, with every generation of it the
    /// store still held.
    pub record: BillingGroupRecord,
    /// The instant the delete was made at; the billing period it falls in is the LAST
    /// one this group existed in ([`Self::deletion_billing_period`]).
    pub deletion_time_millis: i64,
}

impl DeletedBillingGroupRecord {
    /// The billing period the delete was made in — the last period this group billed
    /// in, since a group deleted part-way through a month billed its family for that
    /// month.
    pub fn deletion_billing_period(&self) -> String {
        billing_period_at(self.deletion_time_millis)
    }

    /// Did this group exist in `billing_period`? From the period it was created in
    /// through the one it was deleted in, both inclusive; a value that is not a billing
    /// period at all names no period at all.
    pub fn existed_in(&self, billing_period: &str) -> bool {
        match (
            billing_period_key(billing_period),
            billing_period_key(&self.record.creation_billing_period()),
            billing_period_key(&self.deletion_billing_period()),
        ) {
            (Some(wanted), Some(created), Some(deleted)) => {
                (created..=deleted).contains(&wanted)
            }
            _ => false,
        }
    }

    /// Does a read of the billing groups scoped to `billing_period` still REPORT this
    /// deleted group? From the period it was created in up to, but NOT including, the
    /// period the delete was made in.
    ///
    /// A delete takes effect in the billing period it is made in: that is the period the
    /// memberships the group held end at
    /// ([`release_account`](ComAmazonawsBillingconductorAwsbillingconductorState::release_account)),
    /// so from that period on the group holds no accounts and no read reports it, while
    /// every earlier period it billed in still reports it as it stood then. That is
    /// narrower than [`Self::existed_in`], which spans the periods the group's NAME stays
    /// occupied for — the period of the delete keeps the name the group carried there even
    /// though no read of the groups reports the group itself for it.
    pub fn reported_in(&self, billing_period: &str) -> bool {
        match (
            billing_period_key(billing_period),
            billing_period_key(&self.record.creation_billing_period()),
            billing_period_key(&self.deletion_billing_period()),
        ) {
            (Some(wanted), Some(created), Some(deleted)) => (created..deleted).contains(&wanted),
            _ => false,
        }
    }

    /// The generations of this deleted group a period-scoped read of the billing groups
    /// can still REPORT, oldest first: the generation in force in each of the periods
    /// [`Self::reported_in`] answers for, which is exactly what
    /// [`billing_groups_in`](ComAmazonawsBillingconductorAwsbillingconductorState::billing_groups_in)
    /// projects for this record.
    ///
    /// A generation is in force from its own start period up to the next generation's
    /// ([`VersionedResource::effective_in`]), so a generation that took effect in the
    /// period of the delete or later is reported by no period at all, and a group
    /// created and deleted within ONE period is reported by none of its generations.
    /// The OLDEST generation still kept answers for the periods before it, so it is
    /// reported whenever any period reports the group at all.
    pub fn reported_versions(&self) -> Vec<BillingGroupVersion> {
        let (Some(created), Some(deleted)) = (
            billing_period_key(&self.record.creation_billing_period()),
            billing_period_key(&self.deletion_billing_period()),
        ) else {
            return Vec::new();
        };
        if created >= deleted {
            return Vec::new();
        }
        self.record
            .versions()
            .into_iter()
            .enumerate()
            .filter(|(index, version)| {
                *index == 0
                    || billing_period_key(&version.start_billing_period())
                        .is_some_and(|start| start < deleted)
            })
            .map(|(_, version)| version)
            .collect()
    }

    /// The name this group carried in `billing_period` — the name it OCCUPIED there —
    /// or `None` when it did not exist in that period.
    ///
    /// A rename takes effect in the period it is made in, so the name a period holds is
    /// the one the generation in force then carried
    /// ([`BillingGroupRecord::effective_in`]), not whichever name the group happened to
    /// end with.
    pub fn name_in(&self, billing_period: &str) -> Option<String> {
        if !self.existed_in(billing_period) {
            return None;
        }
        self.record
            .effective_in(billing_period)
            .map(|version| version.name)
    }
}

/// One resource associated to a percentage custom line item, as
/// `BatchAssociateResourcesToCustomLineItem` (or the `AssociatedValues` of the
/// charge details the item was created with) persisted it. The billing period range
/// is the one the associating call named, so `EndBillingPeriod` is reported from the
/// association rather than guessed.
///
/// The periods the association is in effect in are held as a LIST of half-open spans,
/// because a call that changes the association in a bounded billing period range
/// changes it only there: removing a range from the middle of the association leaves
/// the periods on either side of it, which one span cannot hold. The spans are
/// disjoint and in ascending order, so a resource is ONE record however many spans it
/// is in effect over and `AssociationSize` counts it once.
#[derive(Debug, Clone, Default)]
pub struct ResourceAssociationRecord {
    /// The FULL ARN of the associated billing group or custom line item.
    pub arn: String,
    /// The billing periods this association is in effect in — see
    /// [`BillingPeriodSpan`]. An association made with no range at all is the one span
    /// that runs from the period it was made in on ([`Self::new`]). The spans are
    /// disjoint and ascending, and there are at most
    /// [`MAX_BILLING_PERIOD_SPANS`] of them: a caller decides how many
    /// bounded calls to make, and each one can carve one more stretch, so the list is
    /// capped like every other store here.
    pub spans: Vec<BillingPeriodSpan>,
}

/// What taking a range of billing periods out of a resource association did — see
/// [`ResourceAssociationRecord::remove_periods`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodsRemoval {
    /// The billing periods the association is in effect in changed.
    Changed,
    /// The range shares no billing period with the association, so nothing changed —
    /// which is also how a retry of a call that already applied answers.
    Unchanged,
    /// REFUSED: the removal would leave the resource in more separate stretches of
    /// billing periods than [`MAX_BILLING_PERIOD_SPANS`], so nothing was written.
    AtSpanLimit,
}

/// What taking a range of billing periods out of a custom line item did to the store —
/// see
/// [`remove_custom_line_item_periods`](ComAmazonawsBillingconductorAwsbillingconductorState::remove_custom_line_item_periods).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundedRemoval {
    /// The item, and every resource association keyed on it, lost the named billing
    /// periods, and the item still applies in some period the range did not name.
    Narrowed,
    /// The named periods were the last the item applied in, so nothing of it is left:
    /// the record and everything that only existed because of it are gone.
    Emptied,
    /// The range names no billing period the item applied in, so nothing was written —
    /// which is also how a retry of a call that already applied answers.
    Unchanged,
    /// REFUSED: the removal would carve the item, or an association keyed on it, into
    /// more separate stretches of billing periods than [`MAX_BILLING_PERIOD_SPANS`], so
    /// nothing was written.
    AtSpanLimit,
}

/// What putting a range of billing periods INTO a resource association did — see
/// [`ResourceAssociationRecord::add_periods`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodsAddition {
    /// The billing periods the association is in effect in changed.
    Changed,
    /// Every billing period the range names was already in effect, so nothing changed —
    /// which is how a retry of a call that already applied answers.
    Unchanged,
    /// REFUSED: the addition would leave the resource in more separate stretches of
    /// billing periods than [`MAX_BILLING_PERIOD_SPANS`], so nothing was written.
    AtSpanLimit,
}

impl ResourceAssociationRecord {
    /// An association of `arn` in effect over `span` — the billing period range the
    /// associating call named.
    ///
    /// An association is in effect no EARLIER than the billing period the call that made
    /// it was made in, because it did not exist before that period: the bound
    /// [`BillingPeriodSpan::started_now`] fills in, so a call that named no inclusive
    /// start begins in the CURRENT billing period rather than in the unbounded past.
    pub fn new(arn: String, span: BillingPeriodSpan) -> Self {
        let span = BillingPeriodSpan::started_now(span.inclusive_start, span.exclusive_end);
        Self {
            arn,
            spans: vec![span],
        }
    }

    /// An association of `arn` in effect over exactly `spans`.
    ///
    /// For the caller that has already worked out the stretches the association is in
    /// effect over rather than one range to be bounded below — the periods a range and
    /// the resource it names SHARE ([`intersect_billing_periods`]), which can be several
    /// stretches when the named resource applies in several. Those stretches come from
    /// the resource's own spans and a range [`BillingPeriodSpan::started_now`] has
    /// already bounded, so each one begins in a real billing period, as [`Self::new`]
    /// makes sure of for the single-range caller.
    pub fn over(arn: String, spans: Vec<BillingPeriodSpan>) -> Self {
        Self { arn, spans }
    }

    /// `Relationship`, DERIVED from the resource the ARN names — see
    /// [`association_relationship`].
    pub fn relationship(&self) -> &'static str {
        association_relationship(&self.arn)
    }

    /// Is this association in effect during `billing_period`? The start is
    /// inclusive and the end exclusive, as the member names say
    /// (`InclusiveStartBillingPeriod` / `ExclusiveEndBillingPeriod`); an absent end is
    /// open, so an association nothing has ended covers every period from the one it
    /// was made in on ([`Self::new`]).
    pub fn covers(&self, billing_period: &str) -> bool {
        self.spans.iter().any(|span| span.covers(billing_period))
    }

    /// `EndBillingPeriod`: the billing period this association stops applying at,
    /// EXCLUSIVE, absent when it does not stop.
    ///
    /// Every read of an association is scoped to ONE billing period, and it asks about
    /// the association as it stands IN that period — so the stretch covering that period
    /// is the one that ends, and a period no stretch covers has no association to report
    /// an end for.
    pub fn end_billing_period(&self, billing_period: &str) -> Option<String> {
        self.spans
            .iter()
            .find(|span| span.covers(billing_period))
            .and_then(|span| span.exclusive_end.clone())
    }

    /// Take the billing periods `removed` names out of this association, and report
    /// what that did to the periods it is in effect in, through the removal every
    /// store here shares ([`remove_billing_periods`]): a disassociation bounded to a
    /// range takes only the periods that range names. An association left in effect in
    /// no period at all is empty ([`Self::is_empty`]) and its record goes.
    pub fn remove_periods(&mut self, removed: &BillingPeriodSpan) -> PeriodsRemoval {
        remove_billing_periods(&mut self.spans, removed)
    }

    /// Put the billing periods `added` names INTO this association, and report what that
    /// did to the periods it is in effect in, through the addition every store here
    /// shares ([`add_billing_periods`]): an association is in effect over the UNION of
    /// the ranges the calls that associated the resource named, so a call naming periods
    /// this association does not cover puts THOSE periods in effect while one naming
    /// only periods it already covers writes nothing.
    pub fn add_periods(&mut self, added: &BillingPeriodSpan) -> PeriodsAddition {
        add_billing_periods(&mut self.spans, added)
    }

    /// Is this association in effect in no billing period at all, so that nothing of
    /// it is left to report?
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

/// Take the billing periods `removed` names out of the associations of `associations`
/// that `narrowed` picks out, and report what that did to them as a whole.
///
/// Each picked association keeps whatever of itself lies outside the removal and is
/// DROPPED once it is in effect in no billing period at all
/// ([`ResourceAssociationRecord::remove_periods`]), the way the disassociation that ends
/// an association over a range drops it — so an association nothing is left of is not a
/// record a read can report. The associations `narrowed` does not pick are left exactly
/// as they are.
///
/// The whole narrowing is refused at the [`MAX_BILLING_PERIOD_SPANS`] bound, since a
/// call that narrows several associations writes all of them or none: nothing of the list
/// is written until every picked association's removal is through.
pub fn remove_association_periods(
    associations: &mut Vec<ResourceAssociationRecord>,
    removed: &BillingPeriodSpan,
    narrowed: impl Fn(&ResourceAssociationRecord) -> bool,
) -> PeriodsRemoval {
    let mut outcome = PeriodsRemoval::Unchanged;
    let mut kept: Vec<ResourceAssociationRecord> = Vec::with_capacity(associations.len());
    for association in associations.iter() {
        if !narrowed(association) {
            kept.push(association.clone());
            continue;
        }
        let mut association = association.clone();
        match association.remove_periods(removed) {
            PeriodsRemoval::Changed => outcome = PeriodsRemoval::Changed,
            PeriodsRemoval::Unchanged => {}
            PeriodsRemoval::AtSpanLimit => return PeriodsRemoval::AtSpanLimit,
        }
        // An association in effect in no billing period at all is a record with nothing
        // left for a read to report, so it is not kept.
        if !association.is_empty() {
            kept.push(association);
        }
    }
    *associations = kept;
    outcome
}

/// How many of `associations` are in effect in `billing_period`: the `AssociationSize` a
/// read scoped to that period reports.
///
/// Every read reporting that member is scoped to ONE billing period, and whether a
/// resource is associated IN a period is the association record's own question
/// ([`ResourceAssociationRecord::covers`]) — the same one the listing of the associations
/// and the charge computation ask of it — so a record the item holds for periods that do
/// not include the one being read counts for nothing there. Counted here once, over the
/// item's own association records ([`CustomLineItemRecord::association_size`]) whether the
/// read reports the generation in effect now or a frozen one, so no two reads of one
/// period can disagree about the number.
fn associations_in_effect(associations: &[ResourceAssociationRecord], billing_period: &str) -> i64 {
    let in_effect = associations
        .iter()
        .filter(|association| association.covers(billing_period))
        .count();
    i64::try_from(in_effect).unwrap_or(i64::MAX)
}

/// One stretch of time during which an account was a member of a billing group,
/// measured in the `BillingPeriod` months the billing API reports in.
///
/// An account-association read "lists linked accounts that are linked to the payer
/// account for the specified time period" (`ListAccountAssociations`), so the store
/// keeps WHEN each membership held rather than only which group holds the account
/// now. The range is half-open, exactly as every other billing-period range here is
/// (see [`billing_period_range_covers`]): the account is a member from
/// `inclusive_start_billing_period` on, and `exclusive_end_billing_period` is the
/// first period it is no longer one — absent while the membership stands.
///
/// `Default` is deliberately NOT derived, for the reason
/// [`PricingRuleAssociationRecord`] gives: an empty start period is not a billing
/// period, and a record carrying one would be read as unbounded.
#[derive(Debug, Clone)]
pub struct AccountAssociationRecord {
    /// The FULL billing group ARN ([`normalize_billing_group_arn`]).
    pub billing_group_arn: String,
    /// The first billing period the account is a member of that group for.
    pub inclusive_start_billing_period: String,
    /// The first billing period it is no longer one, absent while it still is.
    pub exclusive_end_billing_period: Option<String>,
}

impl AccountAssociationRecord {
    /// Does this membership still stand — has nothing ended it?
    pub fn is_live(&self) -> bool {
        self.exclusive_end_billing_period.is_none()
    }

    /// Was the account a member of this group in `billing_period`? Half-open, as
    /// [`billing_period_range_covers`] reads it, so a membership that began and
    /// ended in the SAME period covers no period at all — which is what a listing
    /// for that period reports, and what the live membership says too.
    pub fn covers(&self, billing_period: &str) -> bool {
        billing_period_range_covers(
            Some(&self.inclusive_start_billing_period),
            self.exclusive_end_billing_period.as_deref(),
            billing_period,
        )
    }
}

/// The billing group one account was a member of in `billing_period`, out of the
/// memberships it has had: the NEWEST one covering that period, so an account
/// released and re-associated within one period is reported in the group it ended up
/// in. `None` when none of them covers it.
///
/// One reverse scan per account, bounded by
/// [`MAX_ACCOUNT_ASSOCIATIONS_PER_ACCOUNT`], and the only place that decides which
/// membership a period reports — so every period-scoped read answers the same way.
fn membership_in<'a>(
    memberships: &'a [AccountAssociationRecord],
    billing_period: &str,
) -> Option<&'a str> {
    memberships
        .iter()
        .rev()
        .find(|membership| membership.covers(billing_period))
        .map(|membership| membership.billing_group_arn.as_str())
}

/// Why a resource is not one a percentage custom line item's charge can be computed
/// over — the fault
/// [`ComAmazonawsBillingconductorAwsbillingconductorState::out_of_scope_association`]
/// reports, and the modelled reason every operation that associates a resource
/// refuses it with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutOfScopeAssociation {
    /// A billing group OTHER than the one the custom line item applies to.
    OtherBillingGroup,
    /// A custom line item of another billing group.
    OtherGroupsCustomLineItem,
}

impl OutOfScopeAssociation {
    /// The `ValidationExceptionReason` the model enumerates for this fault: a billing
    /// group that is not the item's own is a mismatched billing group ARN, and an item
    /// of another group is an illegal resource to associate as a child
    /// (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
    pub fn validation_reason(self) -> &'static str {
        match self {
            Self::OtherBillingGroup => "MISMATCHED_BILLINGGROUP_ARN",
            Self::OtherGroupsCustomLineItem => "ILLEGAL_CHILD_ASSOCIATE_RESOURCE",
        }
    }

    /// Which resource is out of scope and why, for the billing group the custom line
    /// item applies to. The arguments are in the order
    /// [`ComAmazonawsBillingconductorAwsbillingconductorState::out_of_scope_association`]
    /// takes them, so neither call site can transpose them.
    pub fn message(self, billing_group_arn: &str, resource_arn: &str) -> String {
        match self {
            Self::OtherBillingGroup => format!(
                "the billing group {resource_arn} is not the billing group \
                 {billing_group_arn} the custom line item applies to"
            ),
            Self::OtherGroupsCustomLineItem => format!(
                "the custom line item {resource_arn} does not belong to the billing group \
                 {billing_group_arn} the custom line item applies to"
            ),
        }
    }
}

/// A custom line item as `CreateCustomLineItem` persisted it. `AssociationSize` and
/// the percentage/flat distinction are DERIVED from this record (see the methods
/// below), never stored twice.
#[derive(Debug, Clone, Default)]
pub struct CustomLineItemRecord {
    pub arn: String,
    pub name: String,
    pub description: String,
    pub billing_group_arn: String,
    pub account_id: Option<String>,
    /// `CustomLineItemChargeDetails.Type`: `FEE` or `CREDIT`.
    pub charge_type: String,
    /// `ChargeDetails.Flat.ChargeValue`, set for a flat custom line item only.
    pub flat_charge_value: Option<f64>,
    /// `ChargeDetails.Percentage.PercentageValue`, set for a percentage custom line
    /// item only — the kind resources can be associated to.
    pub percentage_value: Option<f64>,
    pub line_item_filters: Vec<crate::ops::types::LineItemFilter>,
    pub computation_rule: Option<String>,
    /// `PresentationDetails.Service`.
    pub presentation_service: Option<String>,
    /// The billing periods this item's CURRENT generation applies in, as the stretches
    /// ([`BillingPeriodSpan`]) a read reports that generation for. An item created with
    /// no `BillingPeriodRange` at all is the one stretch that runs from the billing
    /// period it was created in on ([`BillingPeriodSpan::started_now`]), so an earlier,
    /// already closed period does not report it.
    ///
    /// The periods the item applies in AT ALL are these plus the ones its superseded
    /// generations still hold ([`Self::covers`]), because a write applied from a billing
    /// period on takes only that period and the later ones over.
    ///
    /// It is a LIST because a call that deletes the item in a bounded billing period
    /// range deletes it only there: taking a range out of the middle of the item's
    /// stretch leaves the periods on either side of it, which one span cannot hold.
    /// The spans are disjoint and ascending, and there are at most
    /// [`MAX_BILLING_PERIOD_SPANS`] of them, for the reason that constant gives.
    pub spans: Vec<BillingPeriodSpan>,
    pub tags: BTreeMap<String, String>,
    /// The associated resources, in association order.
    pub associated_resources: Vec<ResourceAssociationRecord>,
    pub creation_time_millis: i64,
    pub last_modified_time_millis: i64,
    /// The instant the item's CURRENT generation took effect: the create that minted
    /// the item, or the later write that opened a new generation of it (see
    /// [`ComAmazonawsBillingconductorAwsbillingconductorState::put_custom_line_item`]).
    /// It is what a version reports as its `StartTime`.
    pub version_start_time_millis: i64,
    /// The generations of this item that later writes SUPERSEDED, oldest first, at
    /// most [`MAX_CUSTOM_LINE_ITEM_VERSIONS`] of them. The generation in effect NOW is
    /// not among them — it is projected from this record by [`Self::current_version`] —
    /// so the history and the live item cannot drift. Held on the record so a deleted
    /// item takes its history with it.
    pub superseded_versions: Vec<CustomLineItemVersion>,
}

impl CustomLineItemRecord {
    /// True for a PERCENTAGE custom line item: the model documents
    /// `CustomLineItemChargeDetails` as containing "only one of `Flat` or
    /// `Percentage`", and a percentage value is "multiplied against the combined
    /// value of its associated resources" — so it is the only kind that has
    /// associated resources at all.
    pub fn is_percentage(&self) -> bool {
        self.percentage_value.is_some()
    }

    /// `AssociationSize`: "The number of resources that are associated to the custom
    /// line item" — in `billing_period`, counted the one way every read of that member
    /// counts it ([`associations_in_effect`]).
    ///
    /// These records are the item's ONLY membership, so this is the count every read
    /// reporting the member takes, whichever generation of the item it reports: an
    /// association record carries the billing periods it is in effect over, so a read of
    /// a period a superseded generation answers for is already answered by the periods
    /// those records hold.
    pub fn association_size(&self, billing_period: &str) -> i64 {
        associations_in_effect(&self.associated_resources, billing_period)
    }

    /// Is this custom line item in effect during `billing_period`? Each stretch a
    /// generation of it applies in is half-open and starts no earlier than the period
    /// the item was created in — see [`billing_period_range_covers`].
    ///
    /// EVERY generation counts, not just the one in effect now: a write applied from a
    /// billing period on leaves the periods before it to the generation it superseded
    /// ([`Self::effective_in`]), so the item is still in effect in them.
    pub fn covers(&self, billing_period: &str) -> bool {
        self.spans.iter().any(|span| span.covers(billing_period))
            || self
                .superseded_versions
                .iter()
                .any(|version| version.covers(billing_period))
    }

    /// The billing periods this item applies in, in ANY generation of it, as the fewest
    /// disjoint ascending stretches that cover exactly them.
    ///
    /// [`Self::covers`] asked of every period at once: a read reports a version of the
    /// item in a period one of these stretches covers and in no other, because every
    /// generation's own stretches are here. It is what another resource's association to
    /// this item may be in effect over
    /// ([`ComAmazonawsBillingconductorAwsbillingconductorState::associable_billing_periods`]),
    /// so the periods an association names the item for are the periods a read reports a
    /// version of it in.
    pub fn applied_periods(&self) -> Vec<BillingPeriodSpan> {
        let mut periods: Vec<BillingPeriodSpan> = Vec::new();
        for span in self
            .superseded_versions
            .iter()
            .flat_map(|version| &version.spans)
            .chain(&self.spans)
        {
            periods = joined_billing_periods(&periods, span);
        }
        periods
    }

    /// The generation of this item that applies in `billing_period` — what a read
    /// scoped to that period reports — or `None` when the item applies in no part of
    /// that period.
    ///
    /// A write takes the billing periods it is applied in OVER from the generation
    /// before it, so the generations hold disjoint stretches of periods and at most one
    /// of them answers for a period; the newest is preferred, so a period two
    /// generations could claim reports the later write.
    pub fn effective_in(&self, billing_period: &str) -> Option<CustomLineItemVersion> {
        let current = self.current_version();
        if current.covers(billing_period) {
            return Some(current);
        }
        self.superseded_versions
            .iter()
            .rev()
            .find(|version| version.covers(billing_period))
            .cloned()
    }

    /// Is this item in effect in no billing period at all — in none of its generations
    /// — so that nothing of it is left for a read to report?
    ///
    /// Which is exactly: no generation of it is a version of it anywhere — the rule
    /// [`Self::versions`] reports by ([`CustomLineItemVersion::applies_anywhere`]), asked
    /// of the live generation's own billing periods rather than of a projection of it, so
    /// the two answers cannot disagree about an item a write emptied.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
            && self
                .superseded_versions
                .iter()
                .all(|version| !version.applies_anywhere())
    }

    /// Take the billing periods `removed` names out of this item, and report what that
    /// did to the periods it applies in, through the removal every store here shares
    /// ([`remove_billing_periods`]): a delete bounded to a range takes only the periods
    /// that range names, so the item keeps the periods on either side of it. An item
    /// left applying in no period at all is empty ([`Self::is_empty`]) and its record
    /// goes.
    ///
    /// EVERY generation loses them, because a period the item was deleted from reports
    /// no version of it at all. Nothing is written unless every generation's removal
    /// stays inside [`MAX_BILLING_PERIOD_SPANS`], so a refused removal leaves the whole
    /// item as it was.
    pub fn remove_periods(&mut self, removed: &BillingPeriodSpan) -> PeriodsRemoval {
        let mut spans = self.spans.clone();
        let mut superseded = self.superseded_versions.clone();
        let mut outcome = PeriodsRemoval::Unchanged;
        for generation in std::iter::once(&mut spans)
            .chain(superseded.iter_mut().map(|version| &mut version.spans))
        {
            match remove_billing_periods(generation, removed) {
                PeriodsRemoval::Changed => outcome = PeriodsRemoval::Changed,
                PeriodsRemoval::Unchanged => {}
                PeriodsRemoval::AtSpanLimit => return PeriodsRemoval::AtSpanLimit,
            }
        }
        if outcome == PeriodsRemoval::Changed {
            // A generation this removal left no billing period to is not a version of
            // the item in any period, so there is nothing for a read to report it for —
            // the rule the write path that supersedes a generation enforces too
            // ([`ComAmazonawsBillingconductorAwsbillingconductorState::put_custom_line_item`]).
            superseded.retain(CustomLineItemVersion::applies_anywhere);
            self.spans = spans;
            self.superseded_versions = superseded;
        }
        outcome
    }

    /// This item's CURRENT generation, as a version snapshot.
    ///
    /// Projected from the live record — through the same `ChargeDetails` derivation and
    /// the same resource associations every other read of the item uses — rather than
    /// stored twice, so the newest version a history read reports is the item a read of
    /// the item itself reports.
    pub fn current_version(&self) -> CustomLineItemVersion {
        CustomLineItemVersion {
            name: self.name.clone(),
            description: self.description.clone(),
            billing_group_arn: self.billing_group_arn.clone(),
            account_id: self.account_id.clone(),
            charge_details: self.charge_details(),
            computation_rule: self.computation_rule.clone(),
            presentation_service: self.presentation_service.clone(),
            spans: self.spans.clone(),
            creation_time_millis: self.creation_time_millis,
            last_modified_time_millis: self.last_modified_time_millis,
            start_time_millis: self.version_start_time_millis,
        }
    }

    /// Do this record and `other` hold the SAME values for every member a write to the
    /// item can change — its name, its description, the charge value of the charge kind
    /// it is, the filters its charge applies through and the billing periods its current
    /// generation applies in?
    ///
    /// What decides whether a write modified the item at all: `LastModifiedTime` is the
    /// instant of the write that left the item as it stands, so a write leaving every one
    /// of these members where it was neither moves that instant nor opens a generation —
    /// the rule the sibling resources' updaters apply
    /// ([`PricingRuleRecord::holds_same_members_as`]). The members no write of this kind
    /// can change — the ARN, the billing group, the charge `Type`, `CreationTime` and the
    /// resource associations the association operations own — are not among them, so they
    /// cannot make a no-op write look like a change.
    ///
    /// The billing periods are compared as the PERIODS they are rather than as the
    /// strings a request spelled them with ([`BillingPeriodSpan::covers_same_periods_as`]),
    /// because one month has two spellings and a write naming the other one leaves the
    /// item applying in exactly the periods it already did.
    pub fn holds_same_members_as(&self, other: &Self) -> bool {
        self.name == other.name
            && self.description == other.description
            && self.flat_charge_value == other.flat_charge_value
            && self.percentage_value == other.percentage_value
            && self.line_item_filters == other.line_item_filters
            && self.spans.len() == other.spans.len()
            && std::iter::zip(&self.spans, &other.spans)
                .all(|(span, theirs)| span.covers_same_periods_as(theirs))
    }

    /// Every generation of this item that is a VERSION of it somewhere, OLDEST FIRST:
    /// the ones later writes superseded, frozen as they stood, and then the one in
    /// effect now.
    ///
    /// A generation applying in no billing period at all is a version of the item in
    /// none of them ([`CustomLineItemVersion::applies_anywhere`]), so it is not among
    /// these: a read of the history reports the same generations a read scoped to a
    /// billing period can find ([`Self::effective_in`]). A write drops the generations
    /// it empties, but the generation in effect NOW is projected from the record itself
    /// and so outlives a write that took its last billing period while an earlier
    /// generation still applies — the one case the rule has to hold here.
    pub fn versions(&self) -> Vec<CustomLineItemVersion> {
        let mut versions = self.superseded_versions.clone();
        versions.push(self.current_version());
        versions.retain(CustomLineItemVersion::applies_anywhere);
        versions
    }

    /// The association this item holds for `arn`, if any — the ONE record the resource
    /// is held by, whatever billing periods it is in effect over.
    ///
    /// Whether the resource is associated IN a billing period is the record's own
    /// question ([`ResourceAssociationRecord::covers`]) and never this lookup's: a
    /// resource the item holds a record for can be in effect in some periods and not in
    /// others, so a caller deciding anything period-scoped asks the record.
    pub fn association_mut(&mut self, arn: &str) -> Option<&mut ResourceAssociationRecord> {
        self.associated_resources
            .iter_mut()
            .find(|association| association.arn == arn)
    }

    /// `ChargeDetails` as this service's surface REPORTS a stored item
    /// (`ListCustomLineItemChargeDetails`).
    ///
    /// "It should contain only one of `Flat` or `Percentage`", so the one the item
    /// holds is reported and the other is absent; the `@required Type` is the one the
    /// item carries; and the filter list is omitted entirely when the item holds none,
    /// because an absent list and an empty one are the same membership.
    ///
    /// It lives on the record so that every operation reporting a custom line item —
    /// `ListCustomLineItems`, `UpdateCustomLineItem` — reports ONE projection and the
    /// two views cannot drift.
    pub fn charge_details(&self) -> crate::ops::types::ListCustomLineItemChargeDetails {
        crate::ops::types::ListCustomLineItemChargeDetails {
            flat: self.flat_charge_value.map(|charge_value| {
                crate::ops::types::ListCustomLineItemFlatChargeDetails { charge_value }
            }),
            percentage: self.percentage_value.map(|percentage_value| {
                crate::ops::types::ListCustomLineItemPercentageChargeDetails { percentage_value }
            }),
            r#type: self.charge_type.clone(),
            line_item_filters: (!self.line_item_filters.is_empty())
                .then(|| self.line_item_filters.clone()),
        }
    }
}

/// One GENERATION of a custom line item — a VERSION of it, as
/// `ListCustomLineItemVersions` reports one.
///
/// A generation is the state the item had while it was in effect, together with the
/// billing period it took effect in and the instant it did. A superseded generation is
/// FROZEN: nothing writes to it again, so the history a read reports is what the item
/// really was, not a recomputation from the item it has since become.
///
/// The resource associations are NOT here, for the reason the sibling versioned resources
/// keep their membership off a generation too ([`PricingPlanVersion`],
/// [`BillingGroupVersion`]): an association record carries the billing periods it is in
/// effect over, so `AssociationSize` is derived per period from the item's own records
/// ([`CustomLineItemRecord::association_size`]) — the records the listing of the
/// associations and the charge computation read — and a write to them cannot leave one
/// read of a period counting a membership another read of the same period denies.
#[derive(Debug, Clone, Default)]
pub struct CustomLineItemVersion {
    pub name: String,
    pub description: String,
    pub billing_group_arn: String,
    pub account_id: Option<String>,
    /// `ChargeDetails` as this generation reported them.
    pub charge_details: crate::ops::types::ListCustomLineItemChargeDetails,
    pub computation_rule: Option<String>,
    /// `PresentationDetails.Service`.
    pub presentation_service: Option<String>,
    /// The billing periods THIS generation applies in, as the stretches
    /// ([`BillingPeriodSpan`]) a read scoped to a period finds it by
    /// ([`CustomLineItemRecord::effective_in`]). A write takes its own periods over
    /// from the generation before it, so the generations' stretches are disjoint and
    /// one is left holding exactly the periods the write did not claim.
    pub spans: Vec<BillingPeriodSpan>,
    /// The item's `CreationTime`, which every generation of it shares.
    pub creation_time_millis: i64,
    pub last_modified_time_millis: i64,
    /// `StartTime`: "The inclusive start time" — the instant this generation took
    /// effect.
    pub start_time_millis: i64,
}

impl CustomLineItemVersion {
    /// Does this generation apply in `billing_period`? Each stretch it applies in is
    /// half-open, and a write that named no billing period range leaves the generation
    /// holding the stretches the item already applied in — see
    /// [`billing_period_range_covers`].
    pub fn covers(&self, billing_period: &str) -> bool {
        self.spans.iter().any(|span| span.covers(billing_period))
    }

    /// Does this generation apply in at least one billing period — is it a VERSION of
    /// the item anywhere?
    ///
    /// A generation is a version of the item in the billing periods it applies in, so a
    /// write that leaves it none leaves no version of the item to report: no read
    /// reports such a generation and no write keeps one.
    pub fn applies_anywhere(&self) -> bool {
        !self.spans.is_empty()
    }

    /// `StartBillingPeriod`: the billing period this generation is in effect FROM,
    /// inclusive.
    ///
    /// The range a write names is the stretch the generation it leaves applies in, and
    /// a write that names none leaves a generation in effect from the billing period
    /// that write itself landed in — so a generation always has a start billing period.
    pub fn start_billing_period(&self) -> String {
        self.spans
            .first()
            .and_then(|span| span.inclusive_start.clone())
            .unwrap_or_else(|| billing_period_at(self.start_time_millis))
    }

    /// The billing period this generation stops applying at, EXCLUSIVE; absent when it
    /// does not stop.
    ///
    /// A `CustomLineItemVersionListElement` carries ONE start and end billing period, so
    /// a generation is reported by the outer bounds of the stretches it applies in;
    /// which periods inside them a read reports the item for is decided by the stretches
    /// themselves ([`Self::covers`]).
    pub fn exclusive_end_billing_period(&self) -> Option<String> {
        self.spans
            .last()
            .and_then(|span| span.exclusive_end.clone())
    }
}

/// The auto billing group creation preference recorded for ONE billing transfer, as
/// `UpdateBillingTransferPreference` persisted it.
///
/// `AutoTransferBillingGroupCreationPreference` carries `Enabled` and — only while the
/// preference is enabled — the `PricingPlanArn` the automatically created billing groups
/// are to compute with. The record adds the instant a call last MODIFIED the preference,
/// which is what `LastModifiedTime` reports: a call that restates the preference already
/// held modified nothing, so it leaves that instant where it was
/// ([`Self::holds_same_preference_as`]). A transfer with NO record is how "the preference
/// has never been set" is represented, so that case reports the documented default and
/// an empty `LastModifiedTime` rather than a stored zero.
#[derive(Debug, Clone, Default)]
pub struct BillingTransferPreferenceRecord {
    /// The `ResponsibilityTransferArn` of the billing transfer the preference applies
    /// to — this record's identity. The modelled pattern admits the full ARN only (no
    /// bare-id spelling), so it needs no normalisation.
    pub responsibility_transfer_arn: String,
    /// `AutoBillingTransferBillingGroupCreation.Enabled`.
    pub enabled: bool,
    /// The FULL pricing plan ARN the preference names, held only while `enabled`:
    /// "This value is required when `Enabled` is `true`, and must be omitted when
    /// `Enabled` is `false`."
    pub pricing_plan_arn: Option<String>,
    pub last_modified_time_millis: i64,
}

impl BillingTransferPreferenceRecord {
    /// Is the preference this record holds the one a request asking for `enabled` /
    /// `pricing_plan_arn` already describes — so that recording it would modify nothing?
    ///
    /// `AutoTransferBillingGroupCreationPreference` IS those two members, so they are the
    /// whole comparison; the transfer the record belongs to is its identity rather than
    /// part of its preference. `pricing_plan_arn` is compared in the one spelling the
    /// store keeps ([`normalize_pricing_plan_arn`]), so a plan named by its bare id and
    /// the same plan named by its full ARN are the same preference.
    pub fn holds_same_preference_as(&self, enabled: bool, pricing_plan_arn: Option<&str>) -> bool {
        self.enabled == enabled && self.pricing_plan_arn.as_deref() == pricing_plan_arn
    }
}

/// ONE billing transfer of an organization, as its `ResponsibilityTransferArn` spells it.
///
/// A billing transfer is an AWS Organizations resource, and the modelled pattern spells
/// its whole identity:
/// `^arn:[a-z0-9][a-z0-9-.]{0,62}:organizations::\d{12}:transfer/o-[a-z0-9]{10,32}/(billing)/(inbound|outbound)/rt-[0-9a-z]{8,32}$`
/// — the organization's management account, the organization, the direction the
/// responsibility travels in and the transfer's own id. EVERY one of those segments is
/// part of the identity, so two ARNs differing in any of them name two different
/// transfers, and a store that holds one of them holds nothing about the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BillingTransferRecord {
    /// The full ARN — this record's identity, and the key the store holds it under.
    pub arn: String,
    /// The account segment: the management account of the organization that owns the
    /// transfer.
    pub management_account_id: String,
    /// The `o-…` organization id the transfer belongs to.
    pub organization_id: String,
    /// `inbound` or `outbound`: which way the billing responsibility travels.
    pub direction: String,
    /// The `rt-…` id of the transfer itself.
    pub transfer_id: String,
}

/// Read a `ResponsibilityTransferArn` into the transfer it names, `None` when the value
/// is not a billing transfer ARN at all.
///
/// Every segment the modelled pattern spells is checked here, because this is the point
/// that decides what transfer a caller-supplied string names: the partition, the
/// `organizations` service with its empty region, the 12-digit management account, and
/// the `transfer/o-<organization>/billing/<direction>/rt-<id>` resource with the
/// organization id, the billing direction and the transfer id the pattern bounds.
pub fn parse_responsibility_transfer_arn(arn: &str) -> Option<BillingTransferRecord> {
    let mut segments = arn.strip_prefix("arn:")?.splitn(5, ':');
    let partition = segments.next()?;
    let service = segments.next()?;
    let region = segments.next()?;
    let account = segments.next()?;
    let resource = segments.next()?;
    if !partition_is_modelled(partition) || service != TRANSFER_ARN_SERVICE || !region.is_empty() {
        return None;
    }
    if account.len() != 12 || !account.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    let mut parts = resource.split('/');
    if parts.next()? != TRANSFER_RESOURCE_TYPE {
        return None;
    }
    let organization_id = parts.next()?;
    let transfer_type = parts.next()?;
    let direction = parts.next()?;
    let transfer_id = parts.next()?;
    if parts.next().is_some() || transfer_type != TRANSFER_TYPE_BILLING {
        return None;
    }
    if !bounded_lowercase_id(organization_id, "o-", 10, 32)
        || !bounded_lowercase_id(transfer_id, "rt-", 8, 32)
        || !TRANSFER_DIRECTIONS.contains(&direction)
    {
        return None;
    }
    Some(BillingTransferRecord {
        arn: arn.to_string(),
        management_account_id: account.to_string(),
        organization_id: organization_id.to_string(),
        direction: direction.to_string(),
        transfer_id: transfer_id.to_string(),
    })
}

/// Whether `partition` is a partition segment the modelled transfer pattern admits
/// (`[a-z0-9][a-z0-9-.]{0,62}`): a lowercase alphanumeric first character, then up to 62
/// more that may also be `-` or `.`.
fn partition_is_modelled(partition: &str) -> bool {
    let mut bytes = partition.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    partition.len() <= 63
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
        })
}

/// Whether `value` is `prefix` followed by `min..=max` lowercase alphanumerics — the form
/// the modelled transfer pattern spells an organization id (`o-[a-z0-9]{10,32}`) and a
/// transfer id (`rt-[0-9a-z]{8,32}`) in.
fn bounded_lowercase_id(value: &str, prefix: &str, min: usize, max: usize) -> bool {
    let Some(body) = value.strip_prefix(prefix) else {
        return false;
    };
    (min..=max).contains(&body.len())
        && body
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// The service segment a billing transfer's ARN carries: a transfer is an AWS
/// Organizations resource, not one of this service's.
const TRANSFER_ARN_SERVICE: &str = "organizations";

/// The resource-type segment of a billing transfer's ARN.
const TRANSFER_RESOURCE_TYPE: &str = "transfer";

/// The one responsibility type the modelled pattern admits.
const TRANSFER_TYPE_BILLING: &str = "billing";

/// The directions a billing transfer runs in, as the modelled pattern enumerates them.
const TRANSFER_DIRECTIONS: &[&str] = &["inbound", "outbound"];

/// Per-service state for com-amazonaws-billingconductor-awsbillingconductor. A handler reaches it through [`service_state`].
///
/// The maps are ordered (`BTreeMap`/`BTreeSet`) so that the list operations page
/// over a stable sequence: a `NextToken` handed back by one call has to name the
/// same cut point on the next one.
///
/// # Growth
///
/// EVERY store here is bounded by an explicit maximum, because a caller decides
/// how many requests to make:
///
/// * `pricing_rules` / `pricing_rule_names` hold one entry per rule, at most
///   [`MAX_PRICING_RULES`], and `DeletePricingRule` reclaims both entries; one rule
///   also carries the generations of it that later writes superseded, at most
///   [`MAX_PRICING_RULE_VERSIONS`], oldest dropped first;
///   `pricing_plans` / `pricing_plan_names` hold one entry per plan, at most
///   [`MAX_PRICING_PLANS`], one plan also carrying the generations of it that later
///   writes superseded, at most [`MAX_PRICING_PLAN_VERSIONS`], oldest dropped first; a
///   plan's own association set at most
///   [`MAX_PRICING_RULES_PER_PRICING_PLAN`], beside at most
///   [`MAX_ENDED_PRICING_RULE_ASSOCIATIONS_PER_PRICING_PLAN`] associations of it that
///   have ENDED, oldest dropped first; `billing_groups` / `billing_group_names` one per group,
///   at most [`MAX_BILLING_GROUPS`], one group also carrying the generations of it that
///   later writes superseded, at most [`MAX_BILLING_GROUP_VERSIONS`], oldest dropped
///   first; a group's membership and its share of
///   `account_associations` at most [`MAX_ACCOUNTS_PER_BILLING_GROUP`]. A create or
///   an association past its cap is REFUSED with the modelled
///   `ServiceLimitExceededException` — never by evicting a stored resource, which a
///   later caller must still be able to read. They are written only on a success
///   path (a replay, a name conflict, a missing pricing plan or a taken account all
///   return before any insert), and
///   [`remove_billing_group`](ComAmazonawsBillingconductorAwsbillingconductorState::remove_billing_group)
///   reclaims a deleted group's share of all of them.
/// * `deleted_billing_groups` keeps the record of the groups a delete removed, so the
///   billing periods they billed in still hold them; at most
///   [`MAX_DELETED_BILLING_GROUPS`], oldest deletion forgotten first, which reads back
///   as a period that never held that group.
/// * `custom_line_items` holds one entry per custom line item, at most
///   [`MAX_CUSTOM_LINE_ITEMS`], and each item at most
///   [`MAX_RESOURCES_PER_CUSTOM_LINE_ITEM`] associated resources — as many as there
///   are resources in scope of its billing group to name. A create past the cap is
///   REFUSED with
///   `ServiceLimitExceededException`, and an association past it fails THAT resource
///   with the modelled `SERVICE_LIMIT_EXCEEDED` reason; a deleted billing group takes
///   its items, and their associations, with it. One association, and one item itself,
///   is in effect over at most [`MAX_BILLING_PERIOD_SPANS`] separate stretches of
///   billing periods, since a call bounded to a range inside one of them splits it:
///   a disassociation that would carve a stretch too many fails THAT resource with the
///   same reason and writes nothing, and a delete that would is refused outright. An
///   item also carries the generations
///   of it that later writes superseded, at most [`MAX_CUSTOM_LINE_ITEM_VERSIONS`],
///   oldest dropped first.
/// * `billing_transfers` holds one entry per billing transfer the organization has, at
///   most [`MAX_BILLING_TRANSFERS`]; nothing in this API creates or deletes one, so a
///   transfer past the cap is refused rather than recorded.
/// * `billing_transfer_preferences` holds one entry per billing transfer whose
///   preference was set, at most [`MAX_BILLING_TRANSFER_PREFERENCES`]; nothing in this
///   API deletes a billing transfer, so that store evicts the oldest-set preference
///   first, which reads back as the "never set" it was before.
/// * `billing_family_account_ids` records the consolidated billing family's own
///   roster, which NO request writes (an account joins a family in AWS Organizations,
///   not here), so it is bounded by whatever recorded it rather than by a cap of this
///   store, and no caller can grow it.
/// * `unassociated_accounts` keeps the released accounts this service has SEEN, beside
///   the family's own roster, so a later `ListAccountAssociations` reports one as
///   `UNMONITORED` even where no roster is known. The family's accounts are reported
///   from the roster itself ([`organization_linked_accounts`]), which no cap of this
///   store evicts; nothing deletes a linked account, so this memory is capped at
///   [`MAX_UNASSOCIATED_ACCOUNTS`] and evicts oldest-first, forgetting the evicted
///   account's billing group memberships with it; one account keeps at most
///   [`MAX_ACCOUNT_ASSOCIATIONS_PER_ACCOUNT`] of them, oldest ENDED membership
///   dropped first.
/// * the idempotency index is capped at [`MAX_IDEMPOTENCY_TOKENS`], evicts
///   oldest-first, and additionally drops the entries that point at a deleted
///   group.
/// * a resource's `tags` map is part of its record, so it is reclaimed with the
///   resource and bounded by [`MAX_TAGS_PER_RESOURCE`] — a `TagResource` whose merge
///   would take one past that cap is refused rather than stored. That cap bounds the
///   `ListTagsForResource` response too, since the response IS the stored map.
///
/// A read never writes. The response side is capped too: every page is at most
/// the request's `MaxResults`, or [`DEFAULT_PAGE_SIZE`] when it names none.
#[derive(Debug, Default)]
pub struct ComAmazonawsBillingconductorAwsbillingconductorState {
    /// Pricing rules by full ARN.
    pub pricing_rules: BTreeMap<String, PricingRuleRecord>,
    /// Pricing rule name -> ARN; the uniqueness index behind the name conflict
    /// ("The names must be unique to each pricing rule").
    pub pricing_rule_names: BTreeMap<String, String>,
    /// Pricing plans by full ARN.
    pub pricing_plans: BTreeMap<String, PricingPlanRecord>,
    /// Pricing plan name -> ARN; the uniqueness index behind the name conflict
    /// ("The names must be unique to each pricing plan").
    pub pricing_plan_names: BTreeMap<String, String>,
    /// Billing groups by full ARN.
    pub billing_groups: BTreeMap<String, BillingGroupRecord>,
    /// The billing groups a delete removed, oldest deletion first, at most
    /// [`MAX_DELETED_BILLING_GROUPS`] of them.
    ///
    /// A billing group bills for a BILLING PERIOD, so deleting it does not erase the
    /// periods it billed in: the record of each deleted group is kept here, which is
    /// what makes a billing period's occupancy of a group's name decidable after the
    /// group is gone (see
    /// [`deleted_billing_group_occupying_name`](ComAmazonawsBillingconductorAwsbillingconductorState::deleted_billing_group_occupying_name))
    /// and what lets a read scoped to one of those periods still report the group as it
    /// stood then (see
    /// [`billing_groups_in`](ComAmazonawsBillingconductorAwsbillingconductorState::billing_groups_in)).
    /// Written only by
    /// [`remove_billing_group`](ComAmazonawsBillingconductorAwsbillingconductorState::remove_billing_group),
    /// which is where the cap is enforced; the oldest deletion is forgotten first,
    /// which reads back as a period that never held that group.
    pub deleted_billing_groups: VecDeque<DeletedBillingGroupRecord>,
    /// Custom line items by full ARN, at most [`MAX_CUSTOM_LINE_ITEMS`]; each one
    /// holds at most [`MAX_RESOURCES_PER_CUSTOM_LINE_ITEM`] associated resources —
    /// one per resource in scope of its billing group.
    /// A custom line item applies to a billing group, so
    /// [`remove_billing_group`](ComAmazonawsBillingconductorAwsbillingconductorState::remove_billing_group)
    /// reclaims the ones that named a deleted group.
    pub custom_line_items: BTreeMap<String, CustomLineItemRecord>,
    /// Billing group name -> ARN ("The billing group name. The names must be unique.").
    pub billing_group_names: BTreeMap<String, String>,
    /// Account id -> the billing group memberships that account has had, oldest
    /// first: the ones that have ended and, last, the one that stands now (at most
    /// one, since an account belongs to at most one billing group). The index that
    /// makes "not associated with another billing group" decidable, the MONITORED
    /// filter computable, and a read scoped to a billing period answerable from the
    /// membership that was in force THEN (through [`membership_in`]). At most
    /// [`MAX_ACCOUNT_ASSOCIATIONS_PER_ACCOUNT`] per account, oldest ended membership
    /// dropped first; a delete ends the memberships the deleted group held.
    ///
    /// A handler writes it only through [`Self::associate_account`] and
    /// [`Self::release_account`], and reads it through
    /// [`Self::account_billing_group`], [`Self::linked_accounts`] and
    /// [`Self::billing_group_sizes_in`], so that cap and the one-live-membership
    /// invariant hold wherever a membership changes.
    pub account_associations: BTreeMap<String, Vec<AccountAssociationRecord>>,
    /// The billing transfers THIS organization has, keyed by full `ResponsibilityTransferArn`,
    /// at most [`MAX_BILLING_TRANSFERS`]. This is the set every operation that addresses a
    /// billing transfer resolves one in, so a transfer no entry here names does not exist
    /// as far as this store is concerned.
    ///
    /// A billing transfer is an AWS Organizations resource, and Billing Conductor creates
    /// none: the transfers an organization has are whatever the Organizations surface
    /// minted for it, so this map is empty until something records one, the same way
    /// `payer_account_id` holds no account until something designates one. Private so a
    /// handler cannot insert past the cap: reach it through
    /// [`Self::billing_transfer`], [`Self::billing_transfer_exists`] and
    /// [`Self::record_billing_transfer`].
    billing_transfers: BTreeMap<String, BillingTransferRecord>,
    /// The auto billing group creation preference of every billing transfer that has
    /// one, keyed by its `ResponsibilityTransferArn`. At most
    /// [`MAX_BILLING_TRANSFER_PREFERENCES`]. Private so the cap below cannot be
    /// bypassed by a handler inserting directly: reach it through
    /// [`Self::billing_transfer_preference`] and
    /// [`Self::set_billing_transfer_preference`].
    billing_transfer_preferences: BTreeMap<String, BillingTransferPreferenceRecord>,
    /// The keys of `billing_transfer_preferences` in first-set order, so the cap evicts
    /// the oldest-set preference rather than an arbitrary one.
    billing_transfer_preference_order: VecDeque<String>,
    /// The bounded tail of linked accounts that belong to no billing group: the ones
    /// a delete released, and a primary account that is not a member of its own
    /// group. They are what `UNMONITORED` reports. Private so the cap below cannot
    /// be bypassed by a handler inserting directly.
    unassociated_accounts: VecDeque<String>,
    /// `<operation>:<ClientToken>` -> the ARN that token's first call minted, so a
    /// retry "completes successfully without performing any further actions".
    /// Private for the same reason as `unassociated_accounts`.
    idempotency_tokens: BTreeMap<String, String>,
    /// The keys of `idempotency_tokens` in insertion order, so the cap evicts the
    /// oldest token rather than an arbitrary one.
    idempotency_order: VecDeque<String>,
    /// Monotonic source of the alphanumeric resource ids the ARN patterns require.
    id_counter: u64,
    /// The account that is the PAYER (management) account of the organization this
    /// store records, if any.
    ///
    /// Billing Conductor is a payer-account-only API: its control plane is authorized
    /// for the payer account of the consolidated billing family alone, and every other
    /// caller is refused `AccessDeniedException` (see
    /// [`Self::is_payer_account`] and [`not_payer_account`]). This emulator presents one
    /// account ([`caller_account_id`]) and emulates no Organizations control plane, so
    /// there is no organization and no payer account to record: the field holds `None`
    /// and the emulated account is a standalone, non-payer account — the caller every
    /// recorded Billing Conductor response was taken from. A test (or a future
    /// Organizations surface) designates a payer account by setting it, which is the
    /// only way the control plane becomes reachable.
    pub payer_account_id: Option<String>,
    /// The ids of the consolidated billing family's linked accounts as THIS store
    /// records them, beside the roster the organization itself records
    /// ([`organization_linked_accounts`]): `Some` once they are recorded, `None` while
    /// they are not.
    ///
    /// Which accounts a consolidated billing family holds is an AWS Organizations
    /// fact — an account joins a family THERE, and this API never adds one — so no
    /// request writes this field and nothing a caller does grows it. The family is
    /// normally read from the Organizations store ([`Self::billing_family`]), and this
    /// field is the seam for an engine that mounts no Organizations surface: it carries
    /// ids alone, so an account known only from here is reported with no
    /// `AccountName`/`AccountEmail` beside its id. It holds `None` by default, for the
    /// reason [`Self::payer_account_id`] holds no account, and while NO roster is known
    /// non-membership is not decidable — the accounts this service has seen stay the
    /// linked accounts it reports ([`Self::linked_accounts`]). Once a roster is known
    /// membership IS decidable: an account no roster holds is outside the family
    /// ([`Self::is_outside_billing_family`]), so an operation that would take it into a
    /// billing group's membership refuses the request
    /// ([`account_not_in_billing_family`]), while every account of the roster is a
    /// linked account this service reports whether or not a billing group holds it.
    pub billing_family_account_ids: Option<BTreeSet<String>>,
}

/// The kind -> store-map dispatch both tag accessors share, written ONCE.
///
/// [`ComAmazonawsBillingconductorAwsbillingconductorState::resource_tags`] and
/// [`ComAmazonawsBillingconductorAwsbillingconductorState::resource_tags_mut`] differ
/// only in the map lookup (`get` or `get_mut`) and in how they borrow the record's
/// `tags`, so the list of kinds lives here: a kind added to [`TaggableResourceKind`] is
/// resolved once and both accessors gain it together, which is what keeps the tags a
/// create stored readable by the read half and writable by the write half of the same
/// tagging surface.
///
/// `$arn` is the tagging `ResourceArn` as the caller spelled it; it is read into the one
/// spelling the maps are keyed by ([`parse_taggable_arn`]) before any map is touched, so
/// both halves resolve one resource from all of its spellings. An ARN that names no
/// resource of this service early-returns `None` from the enclosing function, which is
/// why this expands only inside one returning `Option`.
///
/// `None` means this account holds no such resource. It is NOT the whole of "no such
/// resource exists": a resource Amazon Web Services owns rather than this account is
/// present without any create and stores no tags of the caller's here, which is why the
/// tagging surface decides a resource's OWNER ([`is_aws_owned_taggable_resource`]) before
/// it reads this dispatch's answer as a not-found.
macro_rules! resource_tags_of {
    ($state:expr, $arn:expr, $get:ident $(, $mutability:tt)?) => {{
        let resource = parse_taggable_arn($arn)?;
        match resource.kind {
            TaggableResourceKind::BillingGroup => $state
                .billing_groups
                .$get(&resource.arn)
                .map(|record| & $($mutability)? record.tags),
            TaggableResourceKind::PricingPlan => $state
                .pricing_plans
                .$get(&resource.arn)
                .map(|record| & $($mutability)? record.tags),
            TaggableResourceKind::PricingRule => $state
                .pricing_rules
                .$get(&resource.arn)
                .map(|record| & $($mutability)? record.tags),
            TaggableResourceKind::CustomLineItem => $state
                .custom_line_items
                .$get(&resource.arn)
                .map(|record| & $($mutability)? record.tags),
        }
    }};
}

impl ComAmazonawsBillingconductorAwsbillingconductorState {
    /// Mint the next `width`-character `[a-zA-Z0-9]` resource id. Monotonic, so no
    /// two resources of this service ever share an id.
    pub fn mint_resource_id(&mut self, width: usize) -> String {
        self.id_counter += 1;
        let mut value = self.id_counter;
        let mut id = vec![b'0'; width];
        for slot in id.iter_mut().rev() {
            let alphabet_len = u64::try_from(ID_ALPHABET.len()).unwrap_or(36);
            *slot = ID_ALPHABET[usize::try_from(value % alphabet_len).unwrap_or(0)];
            value /= alphabet_len;
        }
        String::from_utf8(id).unwrap_or_else(|_| "0".repeat(width))
    }

    /// The ARN a previous call with this `ClientToken` minted, if any.
    pub fn replayed_idempotent_arn(&self, operation: &str, token: Option<&str>) -> Option<String> {
        let token = token?;
        self.idempotency_tokens
            .get(&idempotency_key(operation, token))
            .cloned()
    }

    /// Remember which ARN this `ClientToken` minted, evicting the oldest remembered
    /// token once [`MAX_IDEMPOTENCY_TOKENS`] are held.
    pub fn record_idempotent_arn(&mut self, operation: &str, token: Option<&str>, arn: &str) {
        let Some(token) = token else { return };
        let key = idempotency_key(operation, token);
        if self.idempotency_tokens.insert(key.clone(), arn.to_string()).is_none() {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency_tokens.remove(&oldest);
            }
        }
    }

    /// The preference recorded for the billing transfer `arn` names, or `None` when no
    /// call ever set one — the case `GetBillingTransferPreference` reports the
    /// documented default and an empty `LastModifiedTime` for.
    pub fn billing_transfer_preference(
        &self,
        arn: &str,
    ) -> Option<&BillingTransferPreferenceRecord> {
        self.billing_transfer_preferences.get(arn)
    }

    /// Record one billing transfer's preference and hand back what was stored,
    /// forgetting the oldest-set preference once [`MAX_BILLING_TRANSFER_PREFERENCES`]
    /// are held.
    ///
    /// `now` becomes the record's `LastModifiedTime`, so a writer hands a fresh instant
    /// here only for a call that actually modifies the preference
    /// ([`BillingTransferPreferenceRecord::holds_same_preference_as`]).
    pub fn set_billing_transfer_preference(
        &mut self,
        arn: &str,
        enabled: bool,
        pricing_plan_arn: Option<String>,
        now: i64,
    ) -> BillingTransferPreferenceRecord {
        let record = BillingTransferPreferenceRecord {
            responsibility_transfer_arn: arn.to_string(),
            enabled,
            pricing_plan_arn,
            last_modified_time_millis: now,
        };
        if self
            .billing_transfer_preferences
            .insert(arn.to_string(), record.clone())
            .is_none()
        {
            self.billing_transfer_preference_order.push_back(arn.to_string());
        }
        while self.billing_transfer_preference_order.len() > MAX_BILLING_TRANSFER_PREFERENCES {
            if let Some(oldest) = self.billing_transfer_preference_order.pop_front() {
                self.billing_transfer_preferences.remove(&oldest);
                // The eviction is the ONE thing about this store a caller cannot see in
                // a response (the forgotten transfer reads back as "never set"), so it
                // is logged rather than silent. Only the transfer's ARN, which the
                // caller itself supplied, never a secret.
                ::tracing::warn!(
                    responsibility_transfer_arn = %oldest,
                    cap = MAX_BILLING_TRANSFER_PREFERENCES,
                    "the oldest billing transfer preference was forgotten at the store's cap"
                );
            }
        }
        record
    }

    /// The billing transfers whose ENABLED preference names the pricing plan `plan_arn`:
    /// "While a pricing plan is specified in an enabled preference, you can't delete
    /// that pricing plan" (`UpdateBillingTransferPreference`'s own documentation), the
    /// case `ConflictExceptionReason` names
    /// `PRICING_PLAN_REFERENCED_BY_PREFERENCE_DELETE_CONFLICT`.
    ///
    /// A disabled preference names no plan at all, so only the enabled ones can hold a
    /// plan back. Stored plan ARNs are normalised
    /// ([`normalize_pricing_plan_arn`]), so one plan has exactly one spelling here.
    pub fn billing_transfer_preferences_referencing_pricing_plan(
        &self,
        plan_arn: &str,
    ) -> Vec<String> {
        self.billing_transfer_preferences
            .values()
            .filter(|record| record.enabled && record.pricing_plan_arn.as_deref() == Some(plan_arn))
            .map(|record| record.responsibility_transfer_arn.clone())
            .collect()
    }

    /// The billing transfer `arn` names, or `None` when this organization has no such
    /// transfer.
    ///
    /// The whole ARN is the key, so every segment the modelled pattern spells — the
    /// management account, the organization, the direction and the transfer id — has to
    /// match a transfer the store holds. A pattern-conforming ARN is not a transfer.
    ///
    /// Fails CLOSED, like the authorization gate: a store that records no payer account
    /// records no organization, so it owns no transfer, and a transfer whose management
    /// account is not the payer belongs to another organization than the one this store
    /// answers for.
    pub fn billing_transfer(&self, arn: &str) -> Option<&BillingTransferRecord> {
        let record = self.billing_transfers.get(arn)?;
        if self.payer_account_id.as_deref() == Some(record.management_account_id.as_str()) {
            Some(record)
        } else {
            None
        }
    }

    /// Whether this organization has the billing transfer `arn` names — the existence
    /// decision behind the modelled `ResourceNotFoundException` of every operation that
    /// addresses one. See [`Self::billing_transfer`].
    pub fn billing_transfer_exists(&self, arn: &str) -> bool {
        self.billing_transfer(arn).is_some()
    }

    /// Record a billing transfer the organization HAS, answering whether the store holds
    /// it afterwards.
    ///
    /// This is the seam the transfer universe enters the store through: Billing Conductor
    /// creates no billing transfer — an Organizations surface does — so the transfers this
    /// store can answer for are the ones recorded here, and a value that is not a billing
    /// transfer ARN at all records nothing. Recording an already-held transfer changes
    /// nothing.
    ///
    /// A create past [`MAX_BILLING_TRANSFERS`] is REFUSED rather than served by evicting a
    /// transfer, because a transfer the store forgot would start answering not-found for
    /// a resource the organization still has.
    pub fn record_billing_transfer(&mut self, arn: &str) -> bool {
        let Some(record) = parse_responsibility_transfer_arn(arn) else {
            return false;
        };
        if self.billing_transfers.contains_key(&record.arn) {
            return true;
        }
        if self.billing_transfers.len() >= MAX_BILLING_TRANSFERS {
            ::tracing::warn!(
                responsibility_transfer_arn = %record.arn,
                cap = MAX_BILLING_TRANSFERS,
                "a billing transfer was not recorded: the store is at its cap"
            );
            return false;
        }
        self.billing_transfers.insert(record.arn.clone(), record);
        true
    }

    /// Record a linked account this service has SEEN belong to no billing group, so a
    /// later `ListAccountAssociations` reports it as `UNMONITORED` even where no family
    /// roster is known. Capped at [`MAX_UNASSOCIATED_ACCOUNTS`], oldest evicted first.
    ///
    /// An evicted account is forgotten whole: the memberships it once had go with it,
    /// unless it is in a billing group NOW, in which case the membership that stands
    /// is what keeps it reportable. So this memory of past writes is bounded, and it is
    /// only a memory: the family's accounts are reported from the family's own roster
    /// ([`Self::billing_family`]), which no cap of this store evicts, so an account of
    /// the family stays `UNMONITORED` in the listing however many releases followed it.
    pub fn note_unassociated_account(&mut self, account_id: &str) {
        if self
            .unassociated_accounts
            .iter()
            .any(|known| known == account_id)
        {
            return;
        }
        self.unassociated_accounts.push_back(account_id.to_string());
        while self.unassociated_accounts.len() > MAX_UNASSOCIATED_ACCOUNTS {
            let Some(evicted) = self.unassociated_accounts.pop_front() else {
                break;
            };
            if self.account_billing_group(&evicted).is_none() {
                self.account_associations.remove(&evicted);
            }
        }
    }

    /// Associate `account_id` with `group_arn` from the CURRENT billing period on —
    /// the period the associating call is made in, the same sense a pricing rule
    /// association ([`PricingRuleAssociationRecord`]) records.
    ///
    /// At most one membership stands at a time, so the one to end is the newest live
    /// one: a membership of ANOTHER group is ended in this same period, and restating
    /// the membership the account already has changes nothing — the period it began in
    /// is the one it is still in force from.
    pub fn associate_account(&mut self, account_id: &str, group_arn: &str) {
        let billing_period = current_billing_period();
        let memberships = self
            .account_associations
            .entry(account_id.to_string())
            .or_default();
        if let Some(live) = memberships.iter_mut().rev().find(|held| held.is_live()) {
            if live.billing_group_arn == group_arn {
                return;
            }
            live.exclusive_end_billing_period = Some(billing_period.clone());
        }
        memberships.push(AccountAssociationRecord {
            billing_group_arn: group_arn.to_string(),
            inclusive_start_billing_period: billing_period,
            exclusive_end_billing_period: None,
        });
        while memberships.len() > MAX_ACCOUNT_ASSOCIATIONS_PER_ACCOUNT {
            memberships.remove(0);
        }
    }

    /// Release `account_id` from `group_arn`: the membership of that group it still
    /// has ends at the CURRENT billing period, and the account is recorded as
    /// belonging to no billing group — which is what `UNMONITORED` reports. A no-op
    /// on the memberships of an account that group does not hold now, so a release
    /// never rewrites what some other group's membership says.
    pub fn release_account(&mut self, account_id: &str, group_arn: &str) {
        let billing_period = current_billing_period();
        if let Some(live) = self
            .account_associations
            .get_mut(account_id)
            .and_then(|memberships| memberships.iter_mut().rev().find(|held| held.is_live()))
        {
            if live.billing_group_arn == group_arn {
                live.exclusive_end_billing_period = Some(billing_period);
            }
        }
        self.note_unassociated_account(account_id);
    }

    /// The billing group `account_id` belongs to NOW, or `None` when it is in none —
    /// the decision "not associated with another billing group" is made from.
    pub fn account_billing_group(&self, account_id: &str) -> Option<&str> {
        self.account_associations
            .get(account_id)?
            .iter()
            .rev()
            .find(|membership| membership.is_live())
            .map(|membership| membership.billing_group_arn.as_str())
    }

    /// Every linked account this service can report, in account-id order, each with
    /// the billing group it was associated with IN `billing_period` (`None` when it
    /// was in none then).
    ///
    /// The roster is the consolidated billing family's own roster ([`Self::billing_family`]),
    /// together with every account this service has seen, whatever period is asked about: a
    /// linked account stays in the consolidated billing family whether or not it was in a
    /// billing group then, so one that joined a group later is reported for an earlier
    /// period with no group at all, and a family account no group has ever held is reported
    /// with none too.
    ///
    /// `organization` is the family as the organization itself records it, read from the
    /// Organizations store before this store's guard was taken
    /// ([`organization_linked_accounts`]).
    pub fn linked_accounts(
        &self,
        billing_period: &str,
        organization: Option<&BTreeMap<String, LinkedAccountIdentity>>,
    ) -> BTreeMap<String, Option<String>> {
        let mut accounts: BTreeMap<String, Option<String>> = self
            .billing_family(organization)
            .into_iter()
            .flatten()
            .map(|(account_id, _)| (account_id, None))
            .chain(
                self.unassociated_accounts
                    .iter()
                    .map(|account_id| (account_id.clone(), None)),
            )
            .collect();
        for (account_id, memberships) in &self.account_associations {
            let group_arn = membership_in(memberships, billing_period).map(str::to_string);
            accounts.insert(account_id.clone(), group_arn);
        }
        accounts
    }

    /// `Size` — "The number of accounts in the particular billing group" — as of
    /// `billing_period`, for each of the groups in `group_arns` (a group nothing was
    /// a member of then counts zero).
    ///
    /// ONE rule answers every period, the membership index through [`membership_in`],
    /// so the count cannot disagree with the listing that reports the same
    /// memberships. And ONE pass over that index answers every group asked about, so a
    /// listing reporting `Size` for a page of groups never rescans the accounts per
    /// group: the work is a single bounded pass, whatever period is asked about.
    pub fn billing_group_sizes_in(
        &self,
        group_arns: &[String],
        billing_period: &str,
    ) -> BTreeMap<String, i64> {
        let mut sizes: BTreeMap<String, i64> = group_arns
            .iter()
            .map(|group_arn| (group_arn.clone(), 0))
            .collect();
        for memberships in self.account_associations.values() {
            let Some(group_arn) = membership_in(memberships, billing_period) else {
                continue;
            };
            if let Some(size) = sizes.get_mut(group_arn) {
                *size = size.saturating_add(1);
            }
        }
        sizes
    }

    /// Every billing group a read scoped to `billing_period` reports, keyed by full ARN
    /// and paired with the generation of it that was in force in that period.
    ///
    /// Which groups a billing period holds is fixed by THAT period, not by what the store
    /// holds now: a billing group bills a consolidated family for a billing period, so the
    /// groups a period reports are the ones that were billing in it — the stored groups
    /// that existed by then ([`BillingGroupRecord::effective_in`]) together with the groups
    /// a delete has since removed, for the periods before that delete took effect
    /// ([`DeletedBillingGroupRecord::reported_in`]). ONE rule answers every period-scoped
    /// read of a group, so the group a period reports cannot disagree with the membership
    /// the same period reports for it ([`Self::billing_group_sizes_in`],
    /// [`Self::linked_accounts`]): for a period a deleted group held accounts in, the ARN
    /// the account listing reports resolves to the group it names.
    ///
    /// Keyed by ARN, and an ARN carries the monotonic id this service mints, so the
    /// sequence is creation order whether or not a group is still stored, and a `NextToken`
    /// names the same cut point on the next call.
    pub fn billing_groups_in(
        &self,
        billing_period: &str,
    ) -> BTreeMap<String, (&BillingGroupRecord, BillingGroupVersion)> {
        let mut groups: BTreeMap<String, (&BillingGroupRecord, BillingGroupVersion)> =
            BTreeMap::new();
        for deleted in &self.deleted_billing_groups {
            if !deleted.reported_in(billing_period) {
                continue;
            }
            if let Some(version) = deleted.record.effective_in(billing_period) {
                groups.insert(deleted.record.arn.clone(), (&deleted.record, version));
            }
        }
        for (arn, group) in &self.billing_groups {
            if let Some(version) = group.effective_in(billing_period) {
                groups.insert(arn.clone(), (group, version));
            }
        }
        groups
    }

    /// The record a delete kept of the billing group `group_arn`, if any: the NEWEST
    /// deletion of it, the way [`Self::deleted_billing_group_occupying_name`] takes the
    /// newest deletion that held a name.
    ///
    /// How a read that addresses ONE group by ARN reaches a group the store no longer
    /// holds; which periods that record still answers for is
    /// [`DeletedBillingGroupRecord::reported_in`].
    pub fn deleted_billing_group(&self, group_arn: &str) -> Option<&DeletedBillingGroupRecord> {
        self.deleted_billing_groups
            .iter()
            .rev()
            .find(|deleted| deleted.record.arn == group_arn)
    }

    /// Delete a billing group and everything that only existed because of it: its
    /// name index entry, the account memberships it held (which END at the current
    /// billing period, so those accounts become `UNMONITORED` from now on while the
    /// periods they were members in still report the group) and the idempotency
    /// entries that named it. `None` when no such group is stored.
    ///
    /// What the delete does NOT erase is the group's own record of the billing periods
    /// it billed in: the removed record is kept as a
    /// [`DeletedBillingGroupRecord`](crate::state::DeletedBillingGroupRecord), so the
    /// name it occupied in those periods stays occupied for them and a read scoped to one
    /// of the periods before the delete still reports the group as it stood then
    /// ([`Self::billing_groups_in`]).
    pub fn remove_billing_group(&mut self, group_arn: &str) -> Option<BillingGroupRecord> {
        let removed = self.billing_groups.remove(group_arn)?;
        self.billing_group_names.remove(&removed.name);
        self.record_deleted_billing_group(removed.clone(), now_millis());

        // A custom line item "applies to" one billing group, so it cannot outlive
        // it; and an association names a resource that exists, so every association
        // of the deleted group or of one of its items goes with them.
        let orphaned: Vec<String> = self
            .custom_line_items
            .iter()
            .filter(|(_, item)| item.billing_group_arn == group_arn)
            .map(|(arn, _)| arn.clone())
            .collect();
        // Through the SAME removal `DeleteCustomLineItem` performs, so an item taken
        // by its billing group leaves exactly as little behind as one deleted
        // directly.
        for arn in &orphaned {
            self.remove_custom_line_item(arn);
        }
        for item in self.custom_line_items.values_mut() {
            item.associated_resources
                .retain(|association| association.arn != group_arn);
        }

        // The memberships the group held END with it, rather than being forgotten: a
        // read scoped to a billing period the group existed in still has to report
        // the accounts it held then.
        let released: Vec<String> = self
            .account_associations
            .keys()
            .filter(|account_id| self.account_billing_group(account_id) == Some(group_arn))
            .cloned()
            .collect();
        for account_id in released {
            self.release_account(&account_id, group_arn);
        }

        // The tokens that named one of the deleted ITEMS went with each of them
        // ([`Self::remove_custom_line_item`]), so what is left to reclaim here is the
        // group's own.
        self.idempotency_tokens.retain(|_, arn| arn != group_arn);
        let remaining = &self.idempotency_tokens;
        self.idempotency_order
            .retain(|key| remaining.contains_key(key));

        Some(removed)
    }

    /// Keep the record of a deleted billing group, so the billing periods it billed in
    /// still hold it. The only writer of `deleted_billing_groups`, so the cap below
    /// holds wherever a group is deleted.
    ///
    /// Past [`MAX_DELETED_BILLING_GROUPS`] the OLDEST deletion is forgotten, which
    /// reads back the way that period read before the group ever existed; a forgotten
    /// record frees the name it occupied, which is the one thing about this store a
    /// caller cannot see in a response, so it is logged — only the group's ARN, never a
    /// `@sensitive` member such as its name.
    fn record_deleted_billing_group(
        &mut self,
        record: BillingGroupRecord,
        deletion_time_millis: i64,
    ) {
        self.deleted_billing_groups.push_back(DeletedBillingGroupRecord {
            record,
            deletion_time_millis,
        });
        while self.deleted_billing_groups.len() > MAX_DELETED_BILLING_GROUPS {
            let Some(forgotten) = self.deleted_billing_groups.pop_front() else {
                break;
            };
            ::tracing::warn!(
                billing_group_arn = %forgotten.record.arn,
                cap = MAX_DELETED_BILLING_GROUPS,
                "the oldest deleted billing group was forgotten at the store's cap"
            );
        }
    }

    /// The billing group that was DELETED but still occupied `name` in
    /// `billing_period`, if any — the newest such deletion.
    ///
    /// A billing group bills for a billing period, so a period it billed in keeps the
    /// record of it: the name it carried there stays that period's, and only from the
    /// period after the delete is the name free again. The name a period holds is the
    /// one the generation in force then carried
    /// ([`DeletedBillingGroupRecord::name_in`]), so a group renamed before it was
    /// deleted occupies each of its names in the periods it carried them.
    pub fn deleted_billing_group_occupying_name(
        &self,
        name: &str,
        billing_period: &str,
    ) -> Option<&DeletedBillingGroupRecord> {
        self.deleted_billing_groups
            .iter()
            .rev()
            .find(|deleted| deleted.name_in(billing_period).as_deref() == Some(name))
    }

    /// Delete a custom line item and everything that only existed because of it: the
    /// associations other percentage custom line items held TO it (an association
    /// names a resource that exists, and its own associations go with the record) and
    /// the idempotency entries that named it, so a `ClientToken` cannot replay into
    /// the ARN of an item that no longer exists. `None` when no such item is stored.
    pub fn remove_custom_line_item(&mut self, item_arn: &str) -> Option<CustomLineItemRecord> {
        let removed = self.custom_line_items.remove(item_arn)?;

        for item in self.custom_line_items.values_mut() {
            item.associated_resources
                .retain(|association| association.arn != item_arn);
        }

        self.idempotency_tokens.retain(|_, arn| arn != item_arn);
        let remaining = &self.idempotency_tokens;
        self.idempotency_order
            .retain(|key| remaining.contains_key(key));

        Some(removed)
    }

    /// Take the billing periods `removed` names out of the custom line item `item_arn`:
    /// out of every generation of it, and out of every resource association keyed on it.
    /// `None` when no such item is stored.
    ///
    /// A resource association is in effect only in billing periods the resource it names
    /// is there to be associated in, so narrowing the periods a resource applies in
    /// narrows by the same periods every association keyed on that resource — the ones
    /// the resource itself holds and the ones other resources hold TO it — and an
    /// association left in effect in no period at all goes
    /// ([`remove_association_periods`]). It is the bounded form of the rule a delete that
    /// names no range enforces by removing the record outright
    /// ([`Self::remove_custom_line_item`]): a period the item applies in in no generation
    /// of it holds no association of it either, so no read of that period can report an
    /// association to an item it reports no version of. A narrowing that leaves nothing
    /// of the item removes it through that same method, so it leaves exactly as little
    /// behind as the unbounded delete.
    ///
    /// Nothing is written unless the WHOLE narrowing stays inside
    /// [`MAX_BILLING_PERIOD_SPANS`] — every generation of the item and every association
    /// it narrows — so a refused narrowing leaves the store exactly as it was. `now` is
    /// the instant the narrowed item reports as its most recent modification; the items
    /// that merely held an association to it are not modified by it, exactly as they are
    /// not by the unbounded delete.
    pub fn remove_custom_line_item_periods(
        &mut self,
        item_arn: &str,
        removed: &BillingPeriodSpan,
        now: i64,
    ) -> Option<BoundedRemoval> {
        // Every write below lands on a COPY of a stored record, so a removal refused at
        // the span bound — of the item, or of any association keyed on it — can be
        // answered without having written a thing.
        let mut item = self.custom_line_items.get(item_arn)?.clone();
        match item.remove_periods(removed) {
            PeriodsRemoval::Changed => {}
            // The range names no billing period the item applied in, so there is nothing
            // of the item to take out of the associations keyed on it either.
            PeriodsRemoval::Unchanged => return Some(BoundedRemoval::Unchanged),
            PeriodsRemoval::AtSpanLimit => return Some(BoundedRemoval::AtSpanLimit),
        }
        // The associations the item itself holds: in a period the item applies in in no
        // generation, the item is not there to hold a resource associated at all.
        if remove_association_periods(&mut item.associated_resources, removed, |_| true)
            == PeriodsRemoval::AtSpanLimit
        {
            return Some(BoundedRemoval::AtSpanLimit);
        }
        // The associations OTHER percentage items hold to this one.
        let mut holders: Vec<(String, CustomLineItemRecord)> = Vec::new();
        for (arn, holder) in &self.custom_line_items {
            if arn == item_arn
                || !holder
                    .associated_resources
                    .iter()
                    .any(|association| association.arn == item_arn)
            {
                continue;
            }
            let mut holder = holder.clone();
            let outcome = remove_association_periods(
                &mut holder.associated_resources,
                removed,
                |association| association.arn == item_arn,
            );
            if outcome == PeriodsRemoval::AtSpanLimit {
                return Some(BoundedRemoval::AtSpanLimit);
            }
            holders.push((arn.clone(), holder));
        }

        for (arn, holder) in holders {
            self.custom_line_items.insert(arn, holder);
        }
        if item.is_empty() {
            // Nothing of the item is left for a read to report, so the stored record
            // goes — and with it everything that only existed because of it, including
            // the rest of the associations just narrowed.
            self.remove_custom_line_item(item_arn);
            return Some(BoundedRemoval::Emptied);
        }
        // The record changed, so the instant a read reports as its most recent
        // modification advances with it.
        item.last_modified_time_millis = now;
        self.custom_line_items.insert(item_arn.to_string(), item);
        Some(BoundedRemoval::Narrowed)
    }

    /// Write `updated` over the stored custom line item it names as the generation that
    /// applies from `takes_effect_from` on, keeping the generations a read of an earlier
    /// billing period reports.
    ///
    /// A version of a custom line item is the generation of it that applies in a billing
    /// period, so a write APPLIED FROM a billing period takes that period and every
    /// later one over from the generation before it: that generation is frozen holding
    /// exactly the earlier periods, so a read of one of them still reports the item as
    /// it stood then, and this write opens a new generation at `now`. A write that names
    /// no billing period to be applied from (`takes_effect_from` `None`) takes over
    /// wherever the item applies, so it REPLACES the current generation and the item's
    /// history is left as it was.
    ///
    /// The kept history is bounded like every other store here, since a caller decides
    /// how many writes to make: past [`MAX_CUSTOM_LINE_ITEM_VERSIONS`] superseded
    /// generations the OLDEST is dropped, which reads back as a history — and an item —
    /// that starts later, never as a lost current generation, which a later caller must
    /// still see.
    pub fn put_custom_line_item(
        &mut self,
        mut updated: CustomLineItemRecord,
        now: i64,
        takes_effect_from: Option<&str>,
    ) {
        let arn = updated.arn.clone();
        let previous = self.custom_line_items.get(&arn).map(|previous| {
            (
                previous.superseded_versions.clone(),
                previous.current_version(),
            )
        });
        if let Some((mut kept, mut replaced)) = previous {
            // The billing periods this write takes over: the one it is applied from and
            // every later one, which is every period at all when it names none.
            let taken = BillingPeriodSpan::new(takes_effect_from.map(str::to_string), None);
            for generation in kept.iter_mut().chain(std::iter::once(&mut replaced)) {
                remove_billing_periods(&mut generation.spans, &taken);
            }
            // A generation this write left no billing period to is not a version of the
            // item in any period, so there is nothing for a read to report it for.
            kept.retain(CustomLineItemVersion::applies_anywhere);
            if replaced.applies_anywhere() {
                kept.push(replaced);
                updated.version_start_time_millis = now;
            }
            while kept.len() > MAX_CUSTOM_LINE_ITEM_VERSIONS {
                kept.remove(0);
            }
            updated.superseded_versions = kept;
        }
        self.custom_line_items.insert(arn, updated);
    }

    /// Store `updated` as the pricing rule's CURRENT generation, keeping the generation
    /// it replaces when the write takes effect in a DIFFERENT billing period.
    ///
    /// The rule and its bound are [`put_versioned`]'s, which every versioned resource of
    /// this store is written through, so a pricing rule keeps the generations a read of a
    /// closed period is answered from the same way a pricing plan and a billing group do.
    pub fn put_pricing_rule(&mut self, updated: PricingRuleRecord) {
        put_versioned(&mut self.pricing_rules, updated);
    }

    /// Store `updated` as the pricing plan's CURRENT generation, keeping the generation
    /// it replaces when the write takes effect in a DIFFERENT billing period.
    ///
    /// The rule and its bound are [`put_versioned`]'s — see [`Self::put_pricing_rule`].
    pub fn put_pricing_plan(&mut self, updated: PricingPlanRecord) {
        put_versioned(&mut self.pricing_plans, updated);
    }

    /// Store `updated` as the billing group's CURRENT generation, keeping the generation
    /// it replaces when the write takes effect in a DIFFERENT billing period.
    ///
    /// The rule and its bound are [`put_versioned`]'s — see [`Self::put_pricing_rule`].
    pub fn put_billing_group(&mut self, updated: BillingGroupRecord) {
        put_versioned(&mut self.billing_groups, updated);
    }

    /// Delete a pricing rule and the index entries that only existed because of
    /// it: its name-index entry and the idempotency entries that named it, so a
    /// `ClientToken` cannot replay into the ARN of a rule that no longer exists.
    /// `None` when no such rule is stored.
    ///
    /// The associations pricing PLANS hold to the rule are not stripped here, unlike the
    /// associations [`Self::remove_billing_group`] and [`Self::remove_custom_line_item`]
    /// strip: an association names a resource that exists is enforced for a pricing rule
    /// one step earlier, by refusing the delete outright while any billing period reports
    /// an association to it ([`Self::pricing_plans_referencing_rule_in_any_period`]), so
    /// no association a plan still holds can name a rule this removal took away — and the
    /// ended associations a read of a closed period reports keep the periods they really
    /// were in force in. A plan's own associations go with the plan
    /// ([`Self::remove_pricing_plan`]), which is what makes the rule deletable again.
    /// `tests/all/pricing_rule_delete_reader_agreement.rs` holds the readers to it.
    pub fn remove_pricing_rule(&mut self, rule_arn: &str) -> Option<PricingRuleRecord> {
        let removed = self.pricing_rules.remove(rule_arn)?;
        self.pricing_rule_names.remove(&removed.name);

        self.idempotency_tokens.retain(|_, arn| arn != rule_arn);
        let remaining = &self.idempotency_tokens;
        self.idempotency_order
            .retain(|key| remaining.contains_key(key));

        Some(removed)
    }

    /// Delete a pricing plan and the index entries that only existed because of it:
    /// its name-index entry (so the name frees up for a later create) and the
    /// idempotency entries that named it, so a `ClientToken` cannot replay into the
    /// ARN of a plan that no longer exists. `None` when no such plan is stored.
    ///
    /// A plan's rule ASSOCIATIONS live on its own record, so they go with it; the
    /// rules themselves are separate resources and outlive the plan.
    pub fn remove_pricing_plan(&mut self, plan_arn: &str) -> Option<PricingPlanRecord> {
        let removed = self.pricing_plans.remove(plan_arn)?;
        self.pricing_plan_names.remove(&removed.name);

        self.idempotency_tokens.retain(|_, arn| arn != plan_arn);
        let remaining = &self.idempotency_tokens;
        self.idempotency_order
            .retain(|key| remaining.contains_key(key));

        Some(removed)
    }

    /// The ARNs of every billing group that computes with this pricing plan in ANY
    /// billing period a read of this store can still ask about — the association "the
    /// pricing plan must not be associated with any billing groups to delete
    /// successfully" is decided from.
    ///
    /// A group's `ComputationPreference.PricingPlanArn` is a member a write can change,
    /// so each GENERATION of a group carries the plan that generation computes with
    /// ([`BillingGroupVersion::pricing_plan_arn`]) and a period-scoped read reports the
    /// plan of the generation in force then ([`VersionedResource::effective_in`]).
    /// Every generation a period-scoped read of the groups can report is therefore an
    /// association the store can still report, and a plan one of them names is in use
    /// whether or not the LIVE generation still names it — otherwise the plan could be
    /// deleted out from under a closed period that goes on reporting it, leaving an ARN
    /// that resolves to no plan.
    ///
    /// The groups those reads report are the ones
    /// [`Self::billing_groups_in`] answers from: the groups the store still holds, and
    /// the groups a delete has since removed, for the periods before that delete took
    /// effect — so a DELETED group's still-reportable generations
    /// ([`DeletedBillingGroupRecord::reported_versions`]) hold a plan back exactly as a
    /// stored group's do, the way a deleted group's generations still hold the name they
    /// occupied ([`Self::deleted_billing_group_occupying_name`]). A group may name the
    /// plan in either spelling the `PricingPlanArn` pattern allows, so both sides are
    /// normalised. Keyed by ARN, so one group is reported once and the sequence is
    /// creation order whether or not the group is still stored.
    pub fn billing_groups_referencing_pricing_plan(&self, plan_arn: &str) -> Vec<String> {
        let names_the_plan = |versions: &[BillingGroupVersion]| {
            versions
                .iter()
                .any(|version| normalize_pricing_plan_arn(&version.pricing_plan_arn) == plan_arn)
        };
        let stored = self
            .billing_groups
            .iter()
            .filter(|(_, group)| names_the_plan(&group.versions()))
            .map(|(arn, _)| arn.clone());
        let deleted = self
            .deleted_billing_groups
            .iter()
            .filter(|deleted| names_the_plan(&deleted.reported_versions()))
            .map(|deleted| deleted.record.arn.clone());
        stored.chain(deleted).collect::<BTreeSet<String>>().into_iter().collect()
    }

    /// The ARNs of every stored pricing plan that associates this pricing rule in ANY
    /// billing period — the association that holds a rule back from being deleted.
    ///
    /// An association carries the billing periods it is in force over
    /// ([`PricingRuleAssociationRecord`]), and a read scoped to one of those periods
    /// reports it even after a later call ended it, so an ended association is still an
    /// association the store can report — and a rule deleted under one would leave that
    /// period reporting a rule ARN that resolves to nothing. An association that was in
    /// force in no period at all holds nothing back
    /// ([`PricingRuleAssociationRecord::covers_any_period`]), which is also every
    /// association this store drops rather than keeps.
    pub fn pricing_plans_referencing_rule_in_any_period(&self, rule_arn: &str) -> Vec<String> {
        self.pricing_plans
            .iter()
            .filter(|(_, plan)| {
                plan.pricing_rule_associations.iter().any(|association| {
                    association.covers_any_period()
                        && normalize_pricing_rule_arn(&association.arn) == rule_arn
                })
            })
            .map(|(arn, _)| arn.clone())
            .collect()
    }

    /// The ARNs of every stored pricing plan that associates this pricing rule IN
    /// `billing_period`. A plan may name a rule in either spelling the
    /// `PricingRuleArn` pattern allows, so both sides are normalised, and an
    /// association that had not started by `billing_period` is not in force then.
    pub fn pricing_plans_referencing_rule(
        &self,
        rule_arn: &str,
        billing_period: &str,
    ) -> Vec<String> {
        self.pricing_plans
            .iter()
            .filter(|(_, plan)| {
                plan.associated_rule_arns(billing_period)
                    .iter()
                    .any(|arn| arn == rule_arn)
            })
            .map(|(arn, _)| arn.clone())
            .collect()
    }

    /// `AssociatedPricingPlanCount`: "The pricing plans count that this pricing rule
    /// is associated with" — in `billing_period`, the period the read is scoped to.
    pub fn associated_pricing_plan_count(&self, rule_arn: &str, billing_period: &str) -> i64 {
        i64::try_from(self.pricing_plans_referencing_rule(rule_arn, billing_period).len())
            .unwrap_or(i64::MAX)
    }

    /// True when the pricing plan ARN can be used by a billing group: either the
    /// caller created it, or it is one of the AWS-managed plans the
    /// `PricingPlanFullArn` pattern names.
    pub fn pricing_plan_exists(&self, arn: &str) -> bool {
        is_aws_managed_pricing_plan(arn) || self.pricing_plans.contains_key(arn)
    }

    /// The stored resource a `CustomLineItemAssociationElement` names, as its full
    /// ARN, or `None` when no such resource exists.
    ///
    /// The member admits a full `customlineitem` or `billinggroup` ARN and the bare
    /// `[a-zA-Z0-9]{10,12}` id of either, so a bare id is resolved against what is
    /// stored and names the same resource the ARN form does.
    pub fn resolve_association_element(&self, value: &str) -> Option<String> {
        if value.starts_with("arn:") {
            let arn = value.to_string();
            return (self.custom_line_items.contains_key(&arn)
                || self.billing_groups.contains_key(&arn))
            .then_some(arn);
        }
        let item_arn = custom_line_item_arn(value);
        if self.custom_line_items.contains_key(&item_arn) {
            return Some(item_arn);
        }
        let group_arn = billing_group_arn(value);
        self.billing_groups.contains_key(&group_arn).then_some(group_arn)
    }

    /// Why `resource_arn` cannot be an associated resource of a percentage custom
    /// line item that applies to the billing group `billing_group_arn`, or `None`
    /// when it can.
    ///
    /// A percentage value "is multiplied against the combined value of its associated
    /// resources", and the only charges the item's own billing group carries are that
    /// group's own and those of the custom line items of that same group — so a
    /// resource belonging to any other billing group is a charge this item cannot be
    /// a percentage of. `resource_arn` is a FULL ARN, as
    /// [`Self::resolve_association_element`] resolved it, so a resource that is not a
    /// stored custom line item is a billing group.
    pub fn out_of_scope_association(
        &self,
        billing_group_arn: &str,
        resource_arn: &str,
    ) -> Option<OutOfScopeAssociation> {
        match self.custom_line_items.get(resource_arn) {
            Some(item) => (item.billing_group_arn != billing_group_arn)
                .then_some(OutOfScopeAssociation::OtherGroupsCustomLineItem),
            None => (resource_arn != billing_group_arn)
                .then_some(OutOfScopeAssociation::OtherBillingGroup),
        }
    }

    /// The billing periods of `requested` an association naming `resource_arn` may be in
    /// effect over: `requested` BOUNDED to the periods the named resource is there to be
    /// associated in. Empty when the resource applies in none of them, so there is no
    /// association to store at all.
    ///
    /// A resource association is in effect only in billing periods the resource it names
    /// is there to be associated in — the rule the narrowing writer maintains from the
    /// other end ([`Self::remove_custom_line_item_periods`]): a period the named resource
    /// applies in in no generation of it holds no association of it either, so no read of
    /// that period can report an association to an item it reports no version of.
    ///
    /// A named CUSTOM LINE ITEM applies in the billing periods its generations hold
    /// ([`CustomLineItemRecord::applied_periods`]), which is exactly where a read reports
    /// a version of it, so an association naming it is bounded to those. A named BILLING
    /// GROUP is not bounded that way: it carries the charges of the accounts it bills in
    /// every period of the bill a write may still land in — which is why a custom line
    /// item may be created into the previous billing period of a group created in this
    /// one ([`check_billing_period_range_is_modifiable`]) — so a range is in effect over
    /// it as it stands. `resource_arn` is a FULL ARN, as
    /// [`Self::resolve_association_element`] resolved it, so a resource that is not a
    /// stored custom line item is a billing group.
    pub fn associable_billing_periods(
        &self,
        resource_arn: &str,
        requested: &BillingPeriodSpan,
    ) -> Vec<BillingPeriodSpan> {
        match self.custom_line_items.get(resource_arn) {
            Some(item) => intersect_billing_periods(&item.applied_periods(), requested),
            None => vec![requested.clone()],
        }
    }

    /// Whether the billing group `group_arn` bills `account_id` right now.
    ///
    /// Decided from the account's LIVE membership
    /// ([`Self::account_billing_group`]) — the same index `AssociateAccounts` and
    /// `DisassociateAccounts` maintain — so there is one answer to which group an
    /// account is billed in.
    pub fn billing_group_bills_account(&self, group_arn: &str, account_id: &str) -> bool {
        self.account_billing_group(account_id) == Some(group_arn)
    }

    /// The pricing rules associated with the plan `plan_arn` names IN
    /// `billing_period`: FULL rule ARNs, deduplicated and in ARN order, or `None`
    /// when this account has no such plan.
    ///
    /// An association is in force from the billing period it was made in onward, so a
    /// period before it started reports it not at all — see
    /// [`PricingPlanRecord::associated_rule_arns`]. An AWS-managed plan exists without
    /// the caller creating it and carries no rule of the caller's, so it associates an
    /// EMPTY set rather than being unknown. The order is the ARN order the ids are
    /// minted in, so a `NextToken` cut over this sequence names the same point on the
    /// next call.
    pub fn associated_pricing_rule_arns(
        &self,
        plan_arn: &str,
        billing_period: &str,
    ) -> Option<Vec<String>> {
        match self.pricing_plans.get(plan_arn) {
            Some(plan) => Some(plan.associated_rule_arns(billing_period)),
            None => is_aws_managed_pricing_plan(plan_arn).then(Vec::new),
        }
    }

    /// The rules a request's `PricingRuleArns` names, as FULL rule ARNs — or the
    /// modelled refusal the list earns.
    ///
    /// Every operation that takes a list of pricing rule ARNs resolves it here, so one
    /// rule named in two spellings is one rule everywhere and a list that names no real
    /// rule is refused the same way whichever operation carried it:
    ///
    /// * the `PricingRuleArn` pattern accepts the full ARN or the bare id, so each entry
    ///   is normalised FIRST and every decision below is about the RULE rather than the
    ///   spelling ([`normalize_pricing_rule_arn`]);
    /// * one rule named twice — in either spelling — is a fault in the input, which
    ///   `ValidationExceptionReason` enumerates by name (`DUPLICATE_PRICINGRULE_ARNS`);
    /// * there are no AWS-managed pricing rules (the `PricingRuleArn` pattern allows only
    ///   a 12-digit account segment), so what is stored is the whole universe of rules
    ///   and an entry naming none of them is refused with the reason the model
    ///   enumerates for exactly that (`PRICINGRULES_NOT_EXIST`).
    ///
    /// `Err` carries the `(reason, member, message)` of the modelled `ValidationException`
    /// the calling operation refuses the whole request with, so nothing is stored for a
    /// list that is partly resolvable; `field` names the input member that carried the
    /// list.
    pub fn resolve_pricing_rule_arns<'a>(
        &self,
        field: &'static str,
        arns: impl IntoIterator<Item = &'a str>,
    ) -> Result<Vec<String>, ValidationRefusal> {
        let requested: Vec<String> = arns
            .into_iter()
            .map(normalize_pricing_rule_arn)
            .collect::<Vec<String>>();
        if let Some(duplicate) = first_duplicate(&requested) {
            return Err((
                "DUPLICATE_PRICINGRULE_ARNS",
                field,
                format!("the pricing rule {duplicate} is named more than once in this request"),
            ));
        }
        if let Some(missing) = requested
            .iter()
            .find(|arn| !self.pricing_rules.contains_key(*arn))
        {
            return Err((
                "PRICINGRULES_NOT_EXIST",
                field,
                format!("the pricing rule {missing} does not exist"),
            ));
        }
        Ok(requested)
    }

    /// The pricing rule a name is taken by, when it is not `rule_arn`'s own name.
    pub fn conflicting_pricing_rule_name(&self, name: &str, rule_arn: &str) -> Option<&str> {
        self.pricing_rule_names
            .get(name)
            .map(String::as_str)
            .filter(|existing| *existing != rule_arn)
    }

    /// The pricing plan a name is taken by, when it is not `plan_arn`'s own name — the
    /// uniqueness index behind "The name must be unique to each pricing plan", decided
    /// the same way as for a rule so a rename to the name already held is not a
    /// conflict.
    pub fn conflicting_pricing_plan_name(&self, name: &str, plan_arn: &str) -> Option<&str> {
        self.pricing_plan_names
            .get(name)
            .map(String::as_str)
            .filter(|existing| *existing != plan_arn)
    }

    /// The billing group a name is taken by, when it is not `group_arn`'s own name —
    /// the uniqueness index behind "The names must be unique to each billing group",
    /// decided the same way as for a pricing plan so a rename to the name already
    /// held is not a conflict.
    pub fn conflicting_billing_group_name(&self, name: &str, group_arn: &str) -> Option<&str> {
        self.billing_group_names
            .get(name)
            .map(String::as_str)
            .filter(|existing| *existing != group_arn)
    }

    /// The ARNs of the stored billing groups OTHER than `group_arn` that have
    /// automatic account association enabled — what the
    /// [`MAX_AUTO_ASSOCIATE_BILLING_GROUPS`] cap is decided against.
    ///
    /// `group_arn` is the group the request is about, and the empty string for a group
    /// that does not exist yet, so a request that leaves the flag where the addressed
    /// group already has it is never refused by the group's own enablement.
    pub fn other_auto_associate_billing_groups(&self, group_arn: &str) -> Vec<String> {
        self.billing_groups
            .iter()
            .filter(|(arn, group)| group.auto_associates() && arn.as_str() != group_arn)
            .map(|(arn, _)| arn.clone())
            .collect()
    }

    /// The consolidated billing family this store answers for: every linked account of
    /// the payer account, keyed by account id and carrying the identity the account's own
    /// record holds — or `None` when no roster is known at all, so membership of the
    /// family is not decidable.
    ///
    /// Which accounts a family holds, and what each one is called, is an AWS Organizations
    /// fact that no request of this API writes, so the roster the organization itself
    /// records ([`organization_linked_accounts`], passed in as `organization` because it
    /// is read from that store before this one's guard is taken) is the roster this service
    /// reports. Beside it, a store may record the family's ids on its own
    /// ([`Self::billing_family_account_ids`]) — the seam for an engine that mounts no
    /// Organizations surface, which carries ids alone and so no identity beside them.
    /// Either source alone makes the family known; an account held by neither is outside
    /// it.
    pub fn billing_family(
        &self,
        organization: Option<&BTreeMap<String, LinkedAccountIdentity>>,
    ) -> Option<BTreeMap<String, LinkedAccountIdentity>> {
        if organization.is_none() && self.billing_family_account_ids.is_none() {
            return None;
        }
        let mut roster: BTreeMap<String, LinkedAccountIdentity> = self
            .billing_family_account_ids
            .iter()
            .flatten()
            .map(|account_id| (account_id.clone(), LinkedAccountIdentity::default()))
            .collect();
        for (account_id, identity) in organization.into_iter().flatten() {
            roster.insert(account_id.clone(), identity.clone());
        }
        Some(roster)
    }

    /// Whether `account_id` is known NOT to be a linked account of the consolidated
    /// billing family this store answers for — the decision behind "the account IDs must
    /// be a part of the consolidated billing family".
    ///
    /// Refusing an account needs EVIDENCE that it is outside the family, so the answer
    /// comes from the roster alone ([`Self::billing_family`]): a store no roster is known
    /// for knows of no account that it is outside the family, while one whose roster IS
    /// known answers for every account id, since the roster is the family and an id it
    /// does not hold is not in it.
    pub fn is_outside_billing_family(
        &self,
        account_id: &str,
        organization: Option<&BTreeMap<String, LinkedAccountIdentity>>,
    ) -> bool {
        self.billing_family(organization)
            .is_some_and(|roster| !roster.contains_key(account_id))
    }

    /// The group an account is already associated with, when it is not `group_arn`.
    pub fn conflicting_association(&self, account_id: &str, group_arn: &str) -> Option<&str> {
        self.account_billing_group(account_id)
            .filter(|existing| *existing != group_arn)
    }

    /// Whether `account_id` is the payer (management) account of the organization this
    /// store records.
    ///
    /// False whenever no payer account is recorded, which is the store's default (see
    /// [`Self::payer_account_id`]): the Billing Conductor control plane is then
    /// available to nobody, exactly as it is to an account that is not a payer.
    pub fn is_payer_account(&self, account_id: &str) -> bool {
        self.payer_account_id.as_deref() == Some(account_id)
    }

    /// The tags stored for the resource a tagging `ResourceArn` names, or `None` when
    /// this service holds no such resource — the decision every tagging operation's
    /// not-found answer is made from.
    ///
    /// Tags live ON the resource record (the same map `CreateBillingGroup` /
    /// `CreatePricingPlan` / `CreatePricingRule` / `CreateCustomLineItem` persisted their
    /// `Tags` into), so there is one copy of a resource's tags and a delete reclaims them
    /// with the resource. EVERY taggable kind this service persists is resolved by the one
    /// dispatch both halves of the tagging surface share (`resource_tags_of!`): a kind
    /// left unresolved would make the tags its create stored unreadable.
    ///
    /// `None` is this ACCOUNT holding no such resource, which is not the same question as
    /// whether the resource exists: a resource Amazon Web Services owns exists without any
    /// create and keeps no tags here, so a caller addressing one is answered the refusal
    /// its owner earns ([`tagging_access_denied`]) rather than the surface's not-found. A
    /// handler therefore decides the owner first and reads this answer as a not-found only
    /// afterwards.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        resource_tags_of!(self, resource_arn, get)
    }

    /// [`resource_tags`](Self::resource_tags), for a tagging operation that WRITES.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        resource_tags_of!(self, resource_arn, get_mut, mut)
    }

    /// What the custom line items of one billing group put on its PRO FORMA bill in
    /// `billing_period`, summed per product and SIGNED by each item's charge type.
    ///
    /// A custom line item is an adjustment to what the billing group is charged — "a
    /// one-time fixed charge that can be applied to a single billing group", either "a
    /// fee or discount" — so it is part of the pro forma cost and of nothing else. Every
    /// item of the group whose own billing period range covers the period contributes its
    /// charge with the sign of its type ([`charge_sign`]), under the product it is
    /// presented as ([`custom_line_item_product_name`]). An empty map is a billing group
    /// with no item in effect in that period, so its pro forma cost there is its actual
    /// cost.
    ///
    /// The charges are the ones [`Self::custom_line_item_charges`] settles for the whole
    /// group at once, so each item's charge is worked out exactly ONCE per billing period
    /// and the amount reported is decided by the items and associations the store holds
    /// rather than by the order they are read in.
    pub fn proforma_custom_line_item_charges(
        &self,
        group_arn: &str,
        billing_period: &str,
    ) -> BTreeMap<String, f64> {
        let settled = self.custom_line_item_charges(group_arn, billing_period);
        let mut charges: BTreeMap<String, f64> = BTreeMap::new();
        for item in self.custom_line_items.values() {
            if item.billing_group_arn != group_arn || !item.covers(billing_period) {
                continue;
            }
            let charge = charge_sign(&item.charge_type)
                * settled.get(item.arn.as_str()).copied().unwrap_or(0.0);
            *charges
                .entry(custom_line_item_product_name(item).to_string())
                .or_insert(0.0) += charge;
        }
        charges
    }

    /// The UNSIGNED charge each custom line item of one billing group carries in
    /// `billing_period`, by item ARN — the amount its charge details describe, the item's
    /// type being what decides which way it moves the bill.
    ///
    /// "It should contain only one of `Flat` or `Percentage`": a flat item's charge is its
    /// `ChargeValue`, and a percentage item's is its `PercentageValue` "multiplied against
    /// the combined value of its associated resources", counting only the resources whose
    /// association is in effect in that period. An associated resource is another custom
    /// line item, whose own charge it contributes, or a billing group, which contributes
    /// the metered usage of its accounts — of which this emulator records none, as the
    /// `AWSCost` its cost reports answer with says; an ARN naming neither contributes
    /// nothing.
    ///
    /// Which item lends its charge to which is a graph a CALLER arranges, and nothing
    /// stops two percentage items from being associated to each other: an item the
    /// associations lead back round to is a percentage of its OWN charge, which no charge
    /// of the period settles, so it has no charge there and lends none — the answer that
    /// is a property of the items and associations the store holds rather than of the
    /// order they are walked in. Every other item's charge is settled from the items it
    /// is a percentage of, which the components [`settle_charges`] walks in put first.
    ///
    /// Each item's charge is therefore worked out once per billing period and each
    /// association followed once, whatever shape the caller gave the graph, and the walk
    /// is kept on the HEAP rather than on the native call stack, whose depth a long chain
    /// of associations would otherwise decide.
    ///
    /// The charge values are the ones of the GENERATION of the item that applies in that
    /// billing period ([`CustomLineItemRecord::effective_in`]), so a bill of a period a
    /// later write did not claim is computed from what the item charged then — and an
    /// item NO generation of which applies in the period charges nothing there, however
    /// it is reached, so a percentage item associated to it is lent nothing by it.
    fn custom_line_item_charges<'a>(
        &'a self,
        group_arn: &str,
        billing_period: &str,
    ) -> BTreeMap<&'a str, f64> {
        // The items this group's charges are worked out over: its own items, and every
        // stored item those reach through the associations in effect in the period. Each
        // is addressed by its index here, and the resources a percentage item's charge is
        // taken of wait in `associated_arns` until they have been resolved to the items
        // among them.
        let mut index_of: BTreeMap<&'a str, usize> = BTreeMap::new();
        let mut items: Vec<ChargeInput<'a>> = Vec::new();
        let mut associated_arns: Vec<Vec<&'a str>> = Vec::new();
        for item in self.custom_line_items.values() {
            if item.billing_group_arn != group_arn {
                continue;
            }
            let (input, associated) = charge_input(item, billing_period);
            index_of.insert(input.arn, items.len());
            items.push(input);
            associated_arns.push(associated);
        }
        let mut cursor = 0;
        while cursor < items.len() {
            for arn in std::mem::take(&mut associated_arns[cursor]) {
                // A resource that is no stored custom line item of this store lends
                // nothing, so it is no part of the graph at all.
                let Some(associated) = self.custom_line_items.get(arn) else {
                    continue;
                };
                let lender = match index_of.get(associated.arn.as_str()) {
                    Some(lender) => *lender,
                    None => {
                        let (input, reached) = charge_input(associated, billing_period);
                        let lender = items.len();
                        index_of.insert(input.arn, lender);
                        items.push(input);
                        associated_arns.push(reached);
                        lender
                    }
                };
                items[cursor].lenders.push(lender);
            }
            cursor += 1;
        }
        let charges = settle_charges(&items);
        items
            .iter()
            .zip(charges)
            .map(|(item, charge)| (item.arn, charge))
            .collect()
    }
}

/// One custom line item in the graph a billing group's charges are worked out over: how
/// its own charge in that billing period is settled, and which items of the graph lend it
/// theirs.
#[derive(Debug)]
struct ChargeInput<'a> {
    /// The item's full ARN, the identity this service's maps key it by.
    arn: &'a str,
    /// The amount the item carries ITSELF, for an item whose charge needs no other item:
    /// a flat item's `ChargeValue`, and nothing at all for one no generation of which
    /// applies in the period.
    amount: f64,
    /// `PercentageValue`, for a percentage item: its charge is that percentage of the
    /// combined charge of its [`lenders`](Self::lenders).
    percentage: Option<f64>,
    /// The items this one's charge is a percentage of, by index into the graph — one per
    /// association in effect in the period that names a stored custom line item.
    lenders: Vec<usize>,
}

/// How ONE custom line item's charge in `billing_period` is settled, beside the resources
/// a percentage item's charge is taken of, as the associations in effect in that period
/// name them.
fn charge_input<'a>(
    item: &'a CustomLineItemRecord,
    billing_period: &str,
) -> (ChargeInput<'a>, Vec<&'a str>) {
    let settled = |amount: f64, percentage: Option<f64>| ChargeInput {
        arn: item.arn.as_str(),
        amount,
        percentage,
        lenders: Vec::new(),
    };
    let Some(version) = item.effective_in(billing_period) else {
        // No generation of this item applies in the billing period, so the item charges
        // nothing there — the same answer a read scoped to that period gives, which
        // reports no version of it at all. It is reachable here for an ASSOCIATED item
        // only: the group's own items enter the graph whatever period they apply in, and
        // a report counts only the ones in effect. The live charge values are the ones of
        // a generation the period does not hold, so they are no charge of this period and
        // lend a percentage item associated to this one nothing.
        return (settled(0.0, None), Vec::new());
    };
    let details = version.charge_details;
    let Some(percentage) = details.percentage.filter(|_| details.flat.is_none()) else {
        // A flat item's charge is its own `ChargeValue`, and it lends that whatever else
        // the store holds. An item carrying neither charge member is one no create of
        // this service can leave behind, since exactly one of them is required.
        return (
            settled(details.flat.map_or(0.0, |flat| flat.charge_value), None),
            Vec::new(),
        );
    };
    let associated = item
        .associated_resources
        .iter()
        .filter(|association| association.covers(billing_period))
        .map(|association| association.arn.as_str())
        .collect();
    (settled(0.0, Some(percentage.percentage_value)), associated)
}

/// The charge every item of one billing group's graph carries, settled in an order where
/// the items a percentage is taken of are settled before it.
///
/// That order is the one Tarjan's strongly connected components algorithm closes the
/// components of a directed graph in — when a component closes, every component it draws
/// from has closed already — walked ITERATIVELY on the heap, since how deep the graph runs
/// is the caller's to choose. A component holding more than one item, or an item
/// associated to itself, is a set of items the associations lead back round to: each would
/// be a percentage of its own charge, which no charge of the period settles, so each
/// charges nothing. Every other component is one item, settled from the amount it carries
/// itself or as its percentage of what its lenders carry.
///
/// Each item is closed once and each association followed once, so one billing period's
/// charges cost the graph the store holds — at most [`MAX_CUSTOM_LINE_ITEMS`] items of at
/// most [`MAX_RESOURCES_PER_CUSTOM_LINE_ITEM`] associations each — however the caller
/// arranged it.
fn settle_charges(items: &[ChargeInput<'_>]) -> Vec<f64> {
    let count = items.len();
    let mut charges: Vec<f64> = vec![0.0; count];
    // Tarjan's bookkeeping: when each item was reached, the oldest item its own walk can
    // reach, and whether it is waiting on the stack of a component that has not closed.
    let mut reached_at: Vec<Option<usize>> = vec![None; count];
    let mut oldest_reached: Vec<usize> = vec![0; count];
    let mut awaiting_component: Vec<bool> = vec![false; count];
    let mut component_stack: Vec<usize> = Vec::new();
    // The items whose lenders are being reached, each with how many of them have been
    // reached so far — the walk itself, on the heap.
    let mut walk: Vec<(usize, usize)> = Vec::new();
    let mut reached = 0;
    for root in 0..count {
        if reached_at[root].is_some() {
            continue;
        }
        reached_at[root] = Some(reached);
        oldest_reached[root] = reached;
        reached += 1;
        component_stack.push(root);
        awaiting_component[root] = true;
        walk.push((root, 0));
        while let Some((item, cursor)) = walk.pop() {
            if let Some(&lender) = items[item].lenders.get(cursor) {
                walk.push((item, cursor + 1));
                match reached_at[lender] {
                    None => {
                        reached_at[lender] = Some(reached);
                        oldest_reached[lender] = reached;
                        reached += 1;
                        component_stack.push(lender);
                        awaiting_component[lender] = true;
                        walk.push((lender, 0));
                    }
                    // A lender already reached and still awaiting a component is one this
                    // walk came through, so the two are in one component: the item can
                    // reach back as far as that lender was reached.
                    Some(lender_reached_at) => {
                        if awaiting_component[lender] {
                            oldest_reached[item] = oldest_reached[item].min(lender_reached_at);
                        }
                    }
                }
                continue;
            }
            // Every lender of this item has been reached. The item heads a component when
            // nothing its walk reached is older than the item itself.
            if Some(oldest_reached[item]) == reached_at[item] {
                let mut component: Vec<usize> = Vec::new();
                while let Some(member) = component_stack.pop() {
                    awaiting_component[member] = false;
                    component.push(member);
                    if member == item {
                        break;
                    }
                }
                // A component of SEVERAL items, or one item associated to itself, is a
                // set the associations lead back round to: each of its items would be a
                // percentage of its own charge, so none of them has a charge in this
                // period — the nothing every charge here starts as. A component of one
                // item the associations do not lead back to is settled from the amount the
                // item carries itself, or as its percentage of what its lenders carry,
                // which the components they head settled before this one.
                if component.len() == 1 && !items[item].lenders.contains(&item) {
                    charges[item] = match items[item].percentage {
                        None => items[item].amount,
                        Some(percentage) => {
                            let combined: f64 = items[item]
                                .lenders
                                .iter()
                                .map(|lender| charges[*lender])
                                .sum();
                            percentage / 100.0 * combined
                        }
                    };
                }
            }
            // The item the walk came from reaches whatever this one reaches.
            if let Some(&(came_from, _)) = walk.last() {
                oldest_reached[came_from] = oldest_reached[came_from].min(oldest_reached[item]);
            }
        }
    }
    charges
}

/// A `ClientToken` is scoped to the operation that minted it: two operations may
/// legitimately be retried with the same token value.
fn idempotency_key(operation: &str, token: &str) -> String {
    format!("{operation}:{token}")
}

/// How many `ClientToken` -> ARN entries this service remembers.
///
/// A CHOSEN bound, not an AWS limit: AWS's own idempotency windows are finite (a
/// client token stops being recognised after some hours), and this is the
/// emulator's equivalent — the oldest token is forgotten once the cap is reached, so
/// a caller replaying a token it used long ago creates a new resource instead of
/// growing this map without limit.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 1024;

/// How many billing transfers of its organization this service remembers.
///
/// A CHOSEN bound, not an AWS limit: nothing in THIS API creates or deletes a billing
/// transfer, so the set recorded through
/// [`ComAmazonawsBillingconductorAwsbillingconductorState::record_billing_transfer`] is
/// bounded by nothing this store can see. A transfer past the cap is refused rather than
/// recorded, because evicting one would make the store answer not-found for a transfer
/// the organization still has.
pub const MAX_BILLING_TRANSFERS: usize = 1000;

/// How many billing transfer preferences this service remembers.
///
/// A CHOSEN bound, not an AWS limit: a billing transfer is an AWS Organizations
/// resource this API never creates, so the set of transfer ARNs a caller may set a
/// preference for is bounded by nothing this store can see. `UpdateBillingTransferPreference`
/// models no `ServiceLimitExceededException`, so a set past the cap is not refused:
/// the oldest-SET preference is forgotten, exactly as the idempotency index forgets its
/// oldest token ([`MAX_IDEMPOTENCY_TOKENS`]). A forgotten preference reads back as
/// "never set" — the same answer the store gave for that transfer before anybody set
/// one — and only a write ever evicts, so no read changes what a later caller sees.
pub const MAX_BILLING_TRANSFER_PREFERENCES: usize = 1000;

/// How many released (`UNMONITORED`) linked accounts this service remembers.
///
/// A CHOSEN bound, not an AWS limit: nothing in this API deletes a linked account,
/// so the set of accounts that belong to no billing group would otherwise grow with
/// every create-and-delete. The oldest released account is forgotten first; an
/// account that is still associated with a group is never in this tail.
pub const MAX_UNASSOCIATED_ACCOUNTS: usize = 1024;

/// How many billing group memberships this service remembers per linked account:
/// the one that stands now plus the ones that have ended.
///
/// A CHOSEN bound, not an AWS limit: a caller decides how often it associates and
/// disassociates an account, and each cycle is one more ended membership to report a
/// past billing period from. The oldest ENDED membership is dropped first, so a
/// forgotten one reads back the way that period read before anybody recorded it, and
/// the membership in force is never the one evicted. A billing period is a month, so
/// this holds every membership a long history of them could be reported for.
pub const MAX_ACCOUNT_ASSOCIATIONS_PER_ACCOUNT: usize = 100;

/// How many pricing plans this emulator stores for the account.
///
/// A CHOSEN bound, not an AWS limit (the real service's quota is adjustable per
/// account). It exists because nothing in this slice deletes a pricing plan, so
/// without it one caller could grow the store with every request. A create past the
/// cap is REFUSED with the modelled `ServiceLimitExceededException` rather than
/// evicting someone else's resource: a stored resource must stay readable.
pub const MAX_PRICING_PLANS: usize = 1000;

/// How many billing groups this emulator stores for the account. A CHOSEN bound;
/// see [`MAX_PRICING_PLANS`] for why it exists and how it is enforced.
pub const MAX_BILLING_GROUPS: usize = 1000;

/// How many DELETED billing groups this emulator keeps the record of, so a billing
/// period a deleted group billed in still holds it.
///
/// A CHOSEN bound, not an AWS limit: a caller decides how many groups to create and
/// delete, and each delete is one more record to answer a past period from. The
/// OLDEST deletion is forgotten first, which reads back the way that period read
/// before the group existed, and of the same order as [`MAX_BILLING_GROUPS`], since
/// that is how many groups can be deleted at once.
pub const MAX_DELETED_BILLING_GROUPS: usize = 1000;

/// How many of a consolidated billing family's billing groups may have automatic
/// account association enabled at once.
///
/// NOT a chosen bound: `AccountGrouping.AutoAssociate` elects the ONE group that
/// "will automatically associate newly added Amazon Web Services accounts that join
/// your consolidated billing family" (the member's own documentation), and an account
/// belongs to at most one billing group — so two enabled groups would both claim the
/// same joining account and the election would not decide anything. The model names
/// the refusal a request past the cap earns:
/// `ValidationExceptionReason` enumerates `TOO_MANY_AUTO_ASSOCIATE_BILLING_GROUPS`
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`). The cap is on
/// STATE, so every operation that can turn the flag on enforces it, and a group that
/// already has it on may restate it.
pub const MAX_AUTO_ASSOCIATE_BILLING_GROUPS: usize = 1;

/// How many custom line items this emulator stores for the account. A CHOSEN bound;
/// see [`MAX_PRICING_PLANS`] for why it exists and how it is enforced.
pub const MAX_CUSTOM_LINE_ITEMS: usize = 1000;

/// How many resources one percentage custom line item may hold.
///
/// The membership's OWN capacity bound, not a per-request length: an association names
/// a resource that EXISTS and is in scope of the item's own billing group — that group
/// itself, or another custom line item of that same group
/// ([`ComAmazonawsBillingconductorAwsbillingconductorState::out_of_scope_association`])
/// — and one resource is held at most once, so an item can accumulate no more
/// associations than there are such resources to name: the one group plus the other
/// stored items, at most [`MAX_CUSTOM_LINE_ITEMS`] of them. Associations ACCUMULATE
/// across calls, so no `@length` maximum of an input list bounds them: the
/// `ChargeDetails.Percentage.AssociatedValues` of a create expresses the associations
/// of ONE create and `BatchAssociateResourcesToCustomLineItem`'s `ResourceArns` the
/// resources of ONE request (1..30), which is why a membership bound tighter than 30
/// would fail resources the association operation's own contract admits. The bound is
/// still enforced where associations are made — an association past it fails THAT
/// resource with the modelled `SERVICE_LIMIT_EXCEEDED` reason rather than displacing a
/// stored association — so the store stays bounded however the resolution of an ARN to
/// a resource changes.
pub const MAX_RESOURCES_PER_CUSTOM_LINE_ITEM: usize = MAX_CUSTOM_LINE_ITEMS;

/// How many SEPARATE stretches of billing periods one resource may be in effect over:
/// a resource association ([`ResourceAssociationRecord::spans`]) or a custom line item
/// ([`CustomLineItemRecord::spans`]).
///
/// A CHOSEN bound, not an AWS limit: a caller decides how many bounded
/// `BatchDisassociateResourcesFromCustomLineItem` or `DeleteCustomLineItem` calls to
/// make, and one whose range falls inside a stretch the resource already has splits it
/// in two, so the list has to be capped like the kept history is
/// ([`MAX_CUSTOM_LINE_ITEM_VERSIONS`], the same order of magnitude, for the same
/// reason). A call that would carve one more stretch past the cap is REFUSED and writes
/// nothing, never served by coarsening the periods a stored resource is in effect over,
/// which a later read must still report faithfully.
pub const MAX_BILLING_PERIOD_SPANS: usize = 100;

/// How many SUPERSEDED generations of one custom line item this emulator keeps for
/// `ListCustomLineItemVersions` to report.
///
/// A CHOSEN bound, not an AWS limit: a caller decides how many times to update an
/// item, and each update that takes effect in a new billing period adds a generation,
/// so the kept history has to be capped. The OLDEST generation is dropped first, which
/// reads back as a history that begins later — never as the loss of the generation in
/// effect now, which is projected from the item itself and so is always reported.
pub const MAX_CUSTOM_LINE_ITEM_VERSIONS: usize = 100;

/// How many SUPERSEDED generations of one pricing rule this emulator keeps, so that a
/// read scoped to a closed billing period can report the rule as it stood then.
///
/// A CHOSEN bound, not an AWS limit, of the same order as
/// [`MAX_CUSTOM_LINE_ITEM_VERSIONS`] and for the same reason: each write that takes
/// effect in a new billing period adds a generation, so the kept history has to be
/// capped. The OLDEST generation is dropped first, which reads back as that generation's
/// successor answering for the earlier periods — never as the loss of the rule itself,
/// which a read of every period from its creation onward must still report.
pub const MAX_PRICING_RULE_VERSIONS: usize = 100;

/// How many SUPERSEDED generations of one pricing plan this emulator keeps, so that a
/// read scoped to a closed billing period can report the plan as it stood then.
///
/// A CHOSEN bound, not an AWS limit, of the same order as
/// [`MAX_PRICING_RULE_VERSIONS`] and for the same reason: each write that takes effect
/// in a new billing period adds a generation, so the kept history has to be capped. The
/// OLDEST generation is dropped first, which reads back as that generation's successor
/// answering for the earlier periods — never as the loss of the plan itself, which a
/// read of every period from its creation onward must still report.
pub const MAX_PRICING_PLAN_VERSIONS: usize = 100;

/// How many SUPERSEDED generations of one billing group this emulator keeps, so that a
/// read scoped to a closed billing period can report the group as it stood then.
///
/// A CHOSEN bound, not an AWS limit, of the same order as
/// [`MAX_PRICING_RULE_VERSIONS`] and for the same reason: each write that takes effect
/// in a new billing period adds a generation, so the kept history has to be capped. The
/// OLDEST generation is dropped first, which reads back as that generation's successor
/// answering for the earlier periods — never as the loss of the group itself, which a
/// read of every period from its creation onward must still report.
pub const MAX_BILLING_GROUP_VERSIONS: usize = 100;

/// How many pricing rules this emulator stores for the account. A CHOSEN bound;
/// see [`MAX_PRICING_PLANS`] for why it exists and how it is enforced. A rule IS
/// deletable (`DeletePricingRule`), so a caller that reaches the cap can make room
/// by deleting one rather than being stuck.
pub const MAX_PRICING_RULES: usize = 1000;

/// How many pricing rules ONE pricing plan may associate.
///
/// A CHOSEN bound, informed by the model rather than read off it:
/// `CreatePricingPlanInput.PricingRuleArns` (`PricingRuleArnsInput`, len 0..30) is
/// the largest association set a create can express, and
/// `AssociatePricingRules`/`DisassociatePricingRules` move at most 30 rules per call
/// (`PricingRuleArnsNonEmptyInput`, len 1..30) — so holding a stored plan within
/// what a create can express keeps that plan representable. (A documented AWS quota
/// for rules per plan is not in the model; the `@length` maxima above bound a
/// request rather than the membership it accumulates.) An association that would
/// take a plan past the bound is refused with `AssociatePricingRules`'s modelled
/// `ServiceLimitExceededException` — the one operation of the associate/disassociate
/// pair that declares it — rather than displacing a stored association.
pub const MAX_PRICING_RULES_PER_PRICING_PLAN: usize = 30;

/// How many ENDED pricing rule associations one pricing plan keeps, so that a read
/// scoped to a closed billing period can still report the rules the plan associated
/// then ([`PricingPlanRecord::end_rule_associations`]).
///
/// A CHOSEN bound, not an AWS limit, of the same order as [`MAX_PRICING_PLAN_VERSIONS`]
/// and for the same reason: a caller decides how often it associates and disassociates
/// a rule, and each cycle that spans a new billing period leaves one more ended
/// association to report an earlier period from. The OLDEST ended association is
/// dropped first, which reads back the way that period read before anybody recorded it;
/// an association in force is never evicted, so what a read of the current period
/// reports is untouched.
pub const MAX_ENDED_PRICING_RULE_ASSOCIATIONS_PER_PRICING_PLAN: usize = 100;

/// `ModifierPercentage` as the service stores it: "Your entry will be rounded to
/// the nearest 2 decimal places." (the member's own documentation), so the value
/// a later read reports is the rounded one rather than whatever precision the
/// request carried.
pub fn round_modifier_percentage(value: f64) -> f64 {
    if value.is_finite() {
        (value * 100.0).round() / 100.0
    } else {
        value
    }
}

/// How many linked accounts one billing group may hold. A CHOSEN bound; see
/// [`MAX_PRICING_PLANS`]. The model caps one `AssociateAccounts` call at 30 account
/// ids, which bounds a single request but not the membership it accumulates.
pub const MAX_ACCOUNTS_PER_BILLING_GROUP: usize = 1000;

/// The Service Quotas-style service code this service's limit errors carry.
pub const SERVICE_CODE: &str = "billingconductor";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBillingconductorAwsbillingconductorState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBillingconductorAwsbillingconductorState>)
}

/// What a linked account of the consolidated billing family is called, beside its id:
/// the `AccountName` and `AccountEmail` a listing of the family reports for it.
///
/// Both are `Option`, and both absent is the identity of an account no roster carries one
/// for: a name or an email is reported where the account's own record holds it and left
/// absent otherwise, never invented.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkedAccountIdentity {
    /// "The Amazon Web Services account name" (`AccountAssociationsListElement.AccountName`).
    pub account_name: Option<String>,
    /// "The Amazon Web Services account email" (`AccountAssociationsListElement.AccountEmail`).
    pub account_email: Option<String>,
}

/// The linked accounts of the consolidated billing family this engine's organization has,
/// each with the identity the organization records for it — or `None` when Organizations
/// records no organization at all, in which case the family is not known from there.
///
/// An account joins a consolidated billing family in AWS Organizations and no Billing
/// Conductor request puts one there, so the family's roster, and every account's name and
/// email, is state the Organizations control plane owns: it is READ from that store
/// through the one shared engine rather than derived from what this service happens to
/// have been told. The organization's management account is one of its accounts, so it is
/// in the roster too.
///
/// A one-directional, guard-free read of another service's store: it takes that store's
/// lock, holds no other while it does and hands back owned values, so it is called with NO
/// guard of this service's own store held. Nothing is written — membership of an
/// organization is not something the clock makes due.
pub fn organization_linked_accounts<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<BTreeMap<String, LinkedAccountIdentity>> {
    let organizations =
        ::service_com_amazonaws_organizations_awsorganizationsv20161128::state::service_state(
            engine,
        )?;
    let organizations = organizations.lock();
    organizations.organization.as_ref()?;
    Some(
        organizations
            .accounts
            .iter()
            .map(|(account_id, account)| {
                (
                    account_id.clone(),
                    LinkedAccountIdentity {
                        account_name: Some(account.name.clone()),
                        account_email: Some(account.email.clone()),
                    },
                )
            })
            .collect(),
    )
}

/// The authorization decision for one call of this service's payer-gated surface: whether
/// the caller IS the payer account this store records.
///
/// A READ — it takes the store's lock to compare one account id and writes nothing, so a
/// denied request changes nothing a later caller sees.
///
/// Fails CLOSED: a store this process cannot reach records no payer account, and an
/// authorization check that cannot run must not admit the request. The refusal itself is
/// [`not_payer_account`].
pub fn caller_is_payer_account<Eng: Send + Sync + 'static>(engine: &Eng) -> bool {
    service_state(engine).is_some_and(|state| state.lock().is_payer_account(caller_account_id()))
}
