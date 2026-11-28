// SPDX-License-Identifier: CC0-1.0

//! The quantity a metering request reports, and the allocations that split it.
//!
//! Every quantity in this service arrives as a PAIR of members: a whole-number one
//! (`UsageQuantity`, `Quantity`, `AllocatedUsageQuantity`) and a decimal-string one
//! (`UsageQuantityAsDecimal`, `QuantityAsDecimal`,
//! `AllocatedUsageQuantityAsDecimal`). Both members of a pair name the SAME quantity,
//! so this module is the one place a pair becomes a value:
//!
//! * at most one member of a pair may carry the quantity;
//! * the value is held exactly, in units of the smallest decimal step the model's
//!   pattern allows, so two requests that spell one quantity differently (`17`,
//!   `17.0`, `17.00`) compare EQUAL — which is what the de-duplication index and the
//!   `ClientToken` replay compare by;
//! * an allocation splits a quantity, so it must spell its own share in the same
//!   member of the pair as the quantity it splits, and the shares must add up to it.

use std::fmt;

use crate::ops::types::UsageAllocation;

/// The most fraction digits a decimal quantity carries.
///
/// Cited: the pattern every decimal quantity member in this model targets,
/// `^(0|[1-9][0-9]{0,9})(\.[0-9]{1,6})?$`, admits at most six.
pub const QUANTITY_FRACTION_DIGITS: u32 = 6;

/// How many of the smallest decimal steps make one whole unit.
const UNITS_PER_WHOLE: i64 = 10_i64.pow(QUANTITY_FRACTION_DIGITS);

/// A reported usage quantity, held exactly.
///
/// Compared, hashed and summed by VALUE, never by the text or the member it arrived
/// in: `UsageQuantityAsDecimal$doc` — "Quantities are compared by value when checking
/// for duplicate or retried requests, so `17`, `17.0`, and `17.00` are the same
/// quantity, whether you send it in `UsageQuantity` or `UsageQuantityAsDecimal`."
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Quantity {
    /// The quantity in units of 10^-[`QUANTITY_FRACTION_DIGITS`] of a whole unit, so
    /// every value the decimal form can express is held without rounding.
    units: i64,
}

impl Quantity {
    /// No usage at all — what a request that names neither member reports.
    pub const ZERO: Self = Self { units: 0 };

    /// The quantity a whole-number member reports.
    ///
    /// The whole-number members this reads are non-negative by their own `@range(min: 0)`,
    /// so a quantity is never negative; nothing here depends on that beyond the rendering
    /// below, which carries the sign of the value it holds either way.
    #[must_use]
    pub fn from_whole(whole: i32) -> Self {
        Self {
            units: i64::from(whole) * UNITS_PER_WHOLE,
        }
    }

    /// The quantity a decimal-string member reports, or `None` when the text is not
    /// the decimal form (a non-negative number with at most
    /// [`QUANTITY_FRACTION_DIGITS`] fraction digits).
    ///
    /// The member's own `@pattern` is enforced at the boundary before a handler runs, so
    /// this reads the form rather than re-stating the pattern: it takes a leading zero and
    /// more whole digits than the pattern admits, and answers `None` for anything that is
    /// not a decimal number at all or does not fit.
    #[must_use]
    pub fn from_decimal(text: &str) -> Option<Self> {
        let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
        if whole.is_empty() || !whole.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        if text.contains('.')
            && (fraction.is_empty()
                || fraction.len() > QUANTITY_FRACTION_DIGITS as usize
                || !fraction.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return None;
        }
        let units = whole.parse::<i64>().ok()?.checked_mul(UNITS_PER_WHOLE)?;
        if fraction.is_empty() {
            return Some(Self { units });
        }
        let step = 10_i64.pow(
            QUANTITY_FRACTION_DIGITS.checked_sub(u32::try_from(fraction.len()).ok()?)?,
        );
        let fraction = fraction.parse::<i64>().ok()?.checked_mul(step)?;
        units.checked_add(fraction).map(|units| Self { units })
    }

    /// This quantity in units of 10^-[`QUANTITY_FRACTION_DIGITS`].
    #[must_use]
    pub const fn units(self) -> i64 {
        self.units
    }

    /// A total of several quantities as one quantity, saturating rather than wrapping:
    /// a sum too large to hold is reported as the largest quantity there is, which is
    /// unequal to any quantity a single member can carry — exactly what a caller whose
    /// allocations overflow needs to be told.
    #[must_use]
    pub fn saturating_from_units(units: i128) -> Self {
        Self {
            units: i64::try_from(units).unwrap_or(i64::MAX),
        }
    }
}

/// Rendered the way the decimal member spells it: no fraction when the quantity is a
/// whole number, and no trailing zeros when it is not. The sign is the VALUE's, so a
/// quantity below zero renders below zero rather than losing it to the whole part.
impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.units / UNITS_PER_WHOLE;
        let fraction = (self.units % UNITS_PER_WHOLE).abs();
        if fraction == 0 {
            return write!(f, "{whole}");
        }
        let digits = format!("{fraction:0width$}", width = QUANTITY_FRACTION_DIGITS as usize);
        let sign = if self.units < 0 && whole == 0 { "-" } else { "" };
        write!(f, "{sign}{whole}.{}", digits.trim_end_matches('0'))
    }
}

/// The two members that spell one quantity: the whole-number one and the decimal one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuantityMembers {
    pub whole: &'static str,
    pub decimal: &'static str,
}

/// The quantity pair of a `MeterUsage` request.
pub const METER_USAGE_QUANTITY_MEMBERS: QuantityMembers = QuantityMembers {
    whole: "UsageQuantity",
    decimal: "UsageQuantityAsDecimal",
};

/// The quantity pair of one `UsageRecord` in a `BatchMeterUsage` request.
pub const USAGE_RECORD_QUANTITY_MEMBERS: QuantityMembers = QuantityMembers {
    whole: "Quantity",
    decimal: "QuantityAsDecimal",
};

/// The quantity pair of one `UsageAllocation`.
pub const ALLOCATED_QUANTITY_MEMBERS: QuantityMembers = QuantityMembers {
    whole: "AllocatedUsageQuantity",
    decimal: "AllocatedUsageQuantityAsDecimal",
};

/// Which member of a [`QuantityMembers`] pair carried the quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantityForm {
    /// The whole-number member.
    Whole,
    /// The decimal-string member.
    Decimal,
}

impl QuantityForm {
    /// The member of `members` a quantity in this form is spelled in.
    #[must_use]
    pub const fn member(self, members: QuantityMembers) -> &'static str {
        match self {
            Self::Whole => members.whole,
            Self::Decimal => members.decimal,
        }
    }
}

/// A quantity as one member pair reported it: its value, and which of the two members
/// spelled it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportedQuantity {
    pub value: Quantity,
    pub form: QuantityForm,
}

impl ReportedQuantity {
    /// No usage, in the whole-number form a request that named neither member left it.
    pub const ZERO: Self = Self {
        value: Quantity::ZERO,
        form: QuantityForm::Whole,
    };
}

/// The quantity a member pair reports — `Ok(None)` when it names NEITHER member — or
/// why the pair cannot hold together.
///
/// Cited: `MeterUsageRequest$UsageQuantityAsDecimal` — "You can't specify both
/// `UsageQuantity` and `UsageQuantityAsDecimal`" — and the same sentence on
/// `UsageRecord$QuantityAsDecimal` for a record's own pair.
pub fn reported_quantity(
    whole: Option<i32>,
    decimal: Option<&str>,
    members: QuantityMembers,
) -> Result<Option<ReportedQuantity>, String> {
    match (whole, decimal) {
        (Some(_), Some(_)) => Err(format!(
            "You can't specify both {} and {}.",
            members.whole, members.decimal
        )),
        (None, Some(text)) => Quantity::from_decimal(text)
            .map(|value| {
                Some(ReportedQuantity {
                    value,
                    form: QuantityForm::Decimal,
                })
            })
            .ok_or_else(|| {
                format!(
                    "{} must be a non-negative decimal number, for example 17.42.",
                    members.decimal
                )
            }),
        (Some(whole), None) => Ok(Some(ReportedQuantity {
            value: Quantity::from_whole(whole),
            form: QuantityForm::Whole,
        })),
        (None, None) => Ok(None),
    }
}

/// The quantity a request or a record reports, zero when it names neither member.
///
/// Cited: `MeterUsageRequest$UsageQuantity` — "Defaults to 0 if not specified" — and
/// `UsageRecord$Quantity` — "Defaults to 0 if neither `Quantity` nor
/// `QuantityAsDecimal` is specified."
pub fn reported_quantity_or_zero(
    whole: Option<i32>,
    decimal: Option<&str>,
    members: QuantityMembers,
) -> Result<ReportedQuantity, String> {
    Ok(reported_quantity(whole, decimal, members)?.unwrap_or(ReportedQuantity::ZERO))
}

/// The quantity one allocation reports.
///
/// Cited: `UsageAllocation$AllocatedUsageQuantity` — "Each allocation must specify
/// exactly one of `AllocatedUsageQuantity` or `AllocatedUsageQuantityAsDecimal`" — so
/// naming neither, and naming both, are the same mistake.
pub fn allocation_quantity(allocation: &UsageAllocation) -> Result<ReportedQuantity, String> {
    let exactly_one = || {
        format!(
            "Each UsageAllocation must specify exactly one of {} or {}.",
            ALLOCATED_QUANTITY_MEMBERS.whole, ALLOCATED_QUANTITY_MEMBERS.decimal
        )
    };
    if allocation.allocated_usage_quantity.is_some()
        && allocation.allocated_usage_quantity_as_decimal.is_some()
    {
        return Err(exactly_one());
    }
    reported_quantity(
        allocation.allocated_usage_quantity,
        allocation.allocated_usage_quantity_as_decimal.as_deref(),
        ALLOCATED_QUANTITY_MEMBERS,
    )?
    .ok_or_else(exactly_one)
}

/// The tag set of an allocation, normalized so two allocations carrying the same tags
/// in a different order compare equal.
fn allocation_tag_set(allocation: &UsageAllocation) -> Vec<(String, String)> {
    let mut tags: Vec<(String, String)> = allocation
        .tags
        .iter()
        .flatten()
        .map(|tag| (tag.key.clone(), tag.value.clone()))
        .collect();
    tags.sort();
    tags
}

/// Why a set of usage allocations cannot be accepted for the quantity `members`
/// reported, or `None` when it can.
///
/// The rules come from `MeterUsageRequest$UsageAllocations` /
/// `UsageRecord$UsageAllocations`: "The sum of all UsageAllocation quantities must
/// equal the usage quantity of the request ..., and each UsageAllocation must have a
/// unique set of tags (include no tags). Allocations must use the same type of
/// quantity as the request: AllocatedUsageQuantity with UsageQuantity, or
/// AllocatedUsageQuantityAsDecimal with UsageQuantityAsDecimal." A request that
/// carries no allocations at all allocated nothing, which is not an allocation
/// problem. The returned text is what the caller needs to fix the request; the error
/// CODE it is reported under is the modelled `InvalidUsageAllocationsException`.
pub fn allocations_problem(
    allocations: &[UsageAllocation],
    quantity: ReportedQuantity,
    members: QuantityMembers,
) -> Option<String> {
    if allocations.is_empty() {
        return None;
    }
    let mut allocated: i128 = 0;
    for allocation in allocations {
        let reported = match allocation_quantity(allocation) {
            Ok(reported) => reported,
            Err(problem) => return Some(problem),
        };
        if reported.form != quantity.form {
            return Some(format!(
                "Allocations must use the same type of quantity as the usage quantity: {} with {}",
                quantity.form.member(ALLOCATED_QUANTITY_MEMBERS),
                quantity.form.member(members)
            ));
        }
        allocated += i128::from(reported.value.units());
    }
    if allocated != i128::from(quantity.value.units()) {
        return Some(format!(
            "Sum of allocated usage quantities ({}) is not equal to the usage quantity ({})",
            Quantity::saturating_from_units(allocated),
            quantity.value
        ));
    }
    let mut tag_sets: Vec<Vec<(String, String)>> =
        allocations.iter().map(allocation_tag_set).collect();
    tag_sets.sort();
    let supplied = tag_sets.len();
    tag_sets.dedup();
    if tag_sets.len() != supplied {
        return Some(
            "Each UsageAllocation must have a unique set of tags (include no tags)".to_string(),
        );
    }
    None
}
