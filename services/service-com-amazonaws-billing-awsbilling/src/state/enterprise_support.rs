// SPDX-License-Identifier: CC0-1.0

//! The Enterprise Support contract of this emulator's consolidated billing
//! family: the pricing plans the Support profile's charge is computed by, and
//! the method that distributes that charge across the accounts the profile
//! covers — what `GetEnterpriseSupportChargeSummary` reads.
//!
//! The Support-eligible spend the charge is computed FROM is not held here. It
//! is the per-account-month spend in
//! [`ComAmazonawsBillingAwsbillingState::support_eligible_spend`] (see the
//! `support_charges` module) of the accounts the profile covers — the family
//! accounts subscribed to [`SUPPORT_PLAN_ENTERPRISE`] that month — so the
//! profile's total charge, the percentage it works out to and one account's
//! share of it are readings of one store and cannot disagree.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use crate::ops::types::{
    AccessDeniedException, AdditionalCharge, ChargeAccount, ContractAccount, PricingPlan,
    PricingPlanTier,
};

use super::{
    amount, apply_rate, caller_identity_arn, epoch_secs, prorate, rate_decimal, timestamp,
    ComAmazonawsBillingAwsbillingState, SupportEligibleSpend, BILLING_SERVICE_CODE,
    MINOR_UNITS_PER_UNIT,
};

/// `Proportional`, a `supportAllocationMethod` value.
///
/// From the model's `supportAllocationMethod` doc: "Proportional means support
/// charges are distributed to each account in proportion to its eligible
/// Spend."
pub const SUPPORT_ALLOCATION_PROPORTIONAL: &str = "Proportional";

/// `Fixed_Percentage`, the other `supportAllocationMethod` value: "support
/// charges are distributed across accounts according to pre-configured
/// percentages from the contract" (same doc).
pub const SUPPORT_ALLOCATION_FIXED_PERCENTAGE: &str = "Fixed_Percentage";

/// `AWSSupportEnterprise`, the Enterprise Support plan.
///
/// The plan a `support_eligible_spend` record carries is an `AWSSupport`-coded
/// plan name (see [`super::SUPPORT_PLAN_BUSINESS`] for the member doc that
/// lists the ones `supportPlanName` reports); this is the Enterprise plan's
/// code, and a record carrying it is an account-month inside the Enterprise
/// Support profile.
pub const SUPPORT_PLAN_ENTERPRISE: &str = "AWSSupportEnterprise";

/// The `pricingPlanId` of the standard Enterprise Support pricing plan — the
/// plan a profile is billed under when no negotiated plan replaces it.
pub const ENTERPRISE_STANDARD_PLAN_ID: &str = "enterprise-support-standard";

/// The `name` of that plan, the Support plan it prices.
pub const ENTERPRISE_STANDARD_PLAN_NAME: &str = "Enterprise Support";

/// The `resourceType` the modeled `ResourceNotFoundException` reports for a
/// billing month's Enterprise Support charge.
pub const ENTERPRISE_SUPPORT_CHARGE_RESOURCE_TYPE: &str = "enterprisesupportcharge";

/// `None`, a Reserved Instance or Savings Plans treatment method.
///
/// From the model's `supportReservedInstanceTreatmentMethod` doc, which lists
/// the values both treatment methods take: "None (RI fees excluded from
/// Support-eligible spend), Upfront (full upfront RI fees included in month of
/// purchase), Amortized (RI fees spread over commitment term for RIs purchased
/// on or after Support subscription start date), AmortizedCustom (same as
/// Amortized but only for RIs purchased on or after a specified custom start
/// date), AmortizedAll (RI fees amortized for all active RIs including those
/// purchased before Support subscription started)."
pub const SUPPORT_TREATMENT_NONE: &str = "None";

/// `Upfront`: the whole fee is included in the month it was purchased in (see
/// [`SUPPORT_TREATMENT_NONE`] for the member doc that lists the values).
///
/// It is what a stored [`SupportEligibleSpend`] record carries: the record
/// holds one billing month's Reserved Instance and Savings Plan spend and the
/// month's charge is computed from the whole of it, so no part of a fee is
/// spread over any other month.
pub const SUPPORT_TREATMENT_UPFRONT: &str = "Upfront";

/// `Amortized`: the fee is spread over the commitment term of a purchase made
/// on or after the Support subscription started (see
/// [`SUPPORT_TREATMENT_NONE`]).
pub const SUPPORT_TREATMENT_AMORTIZED: &str = "Amortized";

/// `AmortizedCustom`: amortized, but only for purchases made on or after a
/// custom start date (see [`SUPPORT_TREATMENT_NONE`]).
pub const SUPPORT_TREATMENT_AMORTIZED_CUSTOM: &str = "AmortizedCustom";

/// `AmortizedAll`: amortized for every active purchase, including those made
/// before the Support subscription started (see [`SUPPORT_TREATMENT_NONE`]).
pub const SUPPORT_TREATMENT_AMORTIZED_ALL: &str = "AmortizedAll";

/// Every treatment method the model documents, in the order its
/// `supportReservedInstanceTreatmentMethod` doc lists them.
pub const SUPPORT_TREATMENT_METHODS: &[&str] = &[
    SUPPORT_TREATMENT_NONE,
    SUPPORT_TREATMENT_UPFRONT,
    SUPPORT_TREATMENT_AMORTIZED,
    SUPPORT_TREATMENT_AMORTIZED_CUSTOM,
    SUPPORT_TREATMENT_AMORTIZED_ALL,
];

/// The most pricing plans one answer reports, the modeled `PricingPlanList`
/// length maximum (`pricingPlans`, len ..10).
pub const MAX_REPORTED_PRICING_PLANS: usize = 10;

/// The most additional charges one answer reports, the modeled
/// `AdditionalChargeList` length maximum (len ..50) — and so the most this store
/// holds for one billing month.
pub const MAX_ADDITIONAL_SUPPORT_CHARGES: usize = 50;

/// The most contract accounts one answer reports, the modeled
/// `ContractAccountList` length maximum (`contractPayerAccountIds`, len ..2000).
pub const MAX_CONTRACT_ACCOUNTS: usize = 2000;

/// The most charge accounts one answer reports, the modeled `ChargeAccountList`
/// length maximum (`chargedPayerAccountIds`, len ..1000).
pub const MAX_CHARGE_ACCOUNTS: usize = 1000;

/// The most billing months the contract keeps month detail for.
///
/// [`EnterpriseSupportContract::monthly_details`] grows with every billed
/// month, so it needs a ceiling for the same reason the spend store does: an
/// emulator resource bound, not a published AWS quota. A writer past it answers
/// `false` rather than growing the store without a bound.
pub const MAX_ENTERPRISE_SUPPORT_MONTH_DETAILS: usize = 1024;

/// One tier of an Enterprise Support pricing plan, as the modeled
/// `PricingPlanTier` reports it. Amounts are in minor units
/// ([`MINOR_UNITS_PER_UNIT`]) and rates in basis points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterpriseSupportPricingTier {
    /// The spend at which the tier starts (`tierMinimum`).
    pub tier_minimum_minor: i64,
    /// The spend at which it stops (`tierMaximum`); `None` for the last tier,
    /// which prices every unit above the tier before it.
    pub tier_maximum_minor: Option<i64>,
    /// The charge the tier adds whatever the spend inside it is
    /// (`baseCharge`).
    pub base_charge_minor: i64,
    /// The rate the tier charges on the spend it prices, in basis points
    /// (`additionalPercentageOfAggregateCharges`).
    pub additional_rate_basis_points: i64,
    /// The adjustment added to the tier's own charge
    /// (`aggregateChargesAdjustment`).
    pub aggregate_charges_adjustment_minor: i64,
    /// Whether the tier prices only the slice of the spend inside its own
    /// bounds (`incremental`), rather than the whole aggregate spend.
    pub incremental: bool,
}

/// An Enterprise Support pricing plan: the period it is in effect for, the
/// tiers a profile's Support-eligible spend is priced by, the monthly minimum
/// that charge cannot fall below, and the plan discount that reduces it.
///
/// The members mirror the modeled `PricingPlan`, so a reader reports what is
/// persisted rather than recomputing the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterpriseSupportPricingPlan {
    pub pricing_plan_id: String,
    pub name: String,
    pub description: Option<String>,
    /// The first instant the plan is in effect.
    pub start_date: SystemTime,
    /// The instant it stops being in effect, when it has one; `None` is a plan
    /// still in effect.
    pub end_date: Option<SystemTime>,
    /// The plan's discount, in basis points (`planDiscountPercent`).
    pub plan_discount_basis_points: i64,
    /// Whether that discount also reduces a charge the plan minimum raised
    /// (`discountAppliesToMinimumCharge`).
    pub discount_applies_to_minimum_charge: bool,
    /// The monthly minimum charge (`minimumCharge`): a floor on the charge the
    /// tiers price, so a subscribed profile with little spend is still charged
    /// it.
    pub minimum_charge_minor: i64,
    /// Whether the plan prices spend in tiers (`tiered`).
    pub tiered: bool,
    /// The tiers, lowest first.
    pub tiers: Vec<EnterpriseSupportPricingTier>,
}

/// What one billing month's Support-eligible spend is charged under a pricing
/// plan: the charge the plan's tiers and minimum give it, the plan discount
/// that reduces it, and the total left after that discount.
///
/// Its default is no charge at all — what a month the profile was not billed
/// under any plan for carries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EnterpriseSupportCharge {
    /// The charge before the plan discount, in minor units.
    pub gross_minor: i64,
    /// The plan discount, as the positive magnitude of the reduction.
    pub discount_minor: i64,
    /// `gross_minor` less `discount_minor`, never below zero: a discount
    /// reduces a charge, it never turns one into a payment.
    pub total_minor: i64,
}

impl EnterpriseSupportPricingPlan {
    /// The standard Enterprise Support pricing plan.
    ///
    /// Source: the Support plan prices AWS publishes at
    /// <https://aws.amazon.com/premiumsupport/pricing/>. Enterprise Support
    /// costs the greater of 15,000 a month or a tiered percentage of monthly
    /// Support-eligible spend — 10% of the first 150,000, 7% of 150,000 to
    /// 500,000, 5% of 500,000 to 1,000,000 and 3% above 1,000,000 — with no
    /// negotiated discount, so a profile billed under it is charged the
    /// published schedule and nothing else.
    ///
    /// In effect from the epoch and with no end date, so it prices every
    /// billing month a caller can ask about until a negotiated plan is stored
    /// beside it.
    pub fn standard() -> Self {
        Self {
            pricing_plan_id: ENTERPRISE_STANDARD_PLAN_ID.to_string(),
            name: ENTERPRISE_STANDARD_PLAN_NAME.to_string(),
            description: Some(
                "The greater of the monthly minimum and the tiered percentage of monthly \
                 Support-eligible spend."
                    .to_string(),
            ),
            start_date: SystemTime::UNIX_EPOCH,
            end_date: None,
            plan_discount_basis_points: 0,
            discount_applies_to_minimum_charge: false,
            minimum_charge_minor: 15_000 * MINOR_UNITS_PER_UNIT,
            tiered: true,
            tiers: vec![
                EnterpriseSupportPricingTier {
                    tier_minimum_minor: 0,
                    tier_maximum_minor: Some(150_000 * MINOR_UNITS_PER_UNIT),
                    base_charge_minor: 0,
                    additional_rate_basis_points: 1_000,
                    aggregate_charges_adjustment_minor: 0,
                    incremental: true,
                },
                EnterpriseSupportPricingTier {
                    tier_minimum_minor: 150_000 * MINOR_UNITS_PER_UNIT,
                    tier_maximum_minor: Some(500_000 * MINOR_UNITS_PER_UNIT),
                    base_charge_minor: 0,
                    additional_rate_basis_points: 700,
                    aggregate_charges_adjustment_minor: 0,
                    incremental: true,
                },
                EnterpriseSupportPricingTier {
                    tier_minimum_minor: 500_000 * MINOR_UNITS_PER_UNIT,
                    tier_maximum_minor: Some(1_000_000 * MINOR_UNITS_PER_UNIT),
                    base_charge_minor: 0,
                    additional_rate_basis_points: 500,
                    aggregate_charges_adjustment_minor: 0,
                    incremental: true,
                },
                EnterpriseSupportPricingTier {
                    tier_minimum_minor: 1_000_000 * MINOR_UNITS_PER_UNIT,
                    tier_maximum_minor: None,
                    base_charge_minor: 0,
                    additional_rate_basis_points: 300,
                    aggregate_charges_adjustment_minor: 0,
                    incremental: true,
                },
            ],
        }
    }

    /// Whether the plan is in effect at `at_secs` seconds past the epoch: its
    /// start date is at or before that instant, and its end date — when it has
    /// one — after it.
    pub fn covers_instant(&self, at_secs: f64) -> bool {
        epoch_secs(self.start_date) <= at_secs
            && self
                .end_date
                .is_none_or(|end| at_secs < epoch_secs(end))
    }

    /// Whether the plan is in effect anywhere in the half-open period
    /// `from_secs..to_secs`: its own period overlaps that one, so a plan that
    /// starts or ends inside a billing month is in effect for that month even
    /// though it does not cover every instant of it.
    pub fn covers_period(&self, from_secs: f64, to_secs: f64) -> bool {
        epoch_secs(self.start_date) < to_secs
            && self
                .end_date
                .is_none_or(|end| from_secs < epoch_secs(end))
    }

    /// What the plan's tiers price `total_spend_minor` at, before the plan
    /// minimum and the plan discount.
    ///
    /// An incremental tier prices only the slice of the spend that falls
    /// inside its own bounds; a non-incremental one prices the whole spend,
    /// and only when the spend falls inside them. Either way a tier charges
    /// its base charge, plus its rate on the spend it prices, plus its
    /// aggregate adjustment — and a tier the spend does not reach prices
    /// nothing.
    pub fn tiered_charge_minor(&self, total_spend_minor: i64) -> i64 {
        let mut charge_minor = 0_i64;
        for tier in &self.tiers {
            if total_spend_minor <= tier.tier_minimum_minor && tier.tier_minimum_minor > 0 {
                continue;
            }
            let priced_minor = if tier.incremental {
                let upper_minor = tier
                    .tier_maximum_minor
                    .map_or(total_spend_minor, |maximum| maximum.min(total_spend_minor));
                (upper_minor.saturating_sub(tier.tier_minimum_minor)).max(0)
            } else if tier
                .tier_maximum_minor
                .is_none_or(|maximum| total_spend_minor <= maximum)
            {
                total_spend_minor
            } else {
                continue;
            };
            charge_minor = charge_minor
                .saturating_add(tier.base_charge_minor)
                .saturating_add(apply_rate(priced_minor, tier.additional_rate_basis_points))
                .saturating_add(tier.aggregate_charges_adjustment_minor);
        }
        charge_minor
    }

    /// The charge for `total_spend_minor` under this plan: the greater of what
    /// its tiers price the spend at and its monthly minimum, less the plan
    /// discount — which reduces a charge the minimum raised only when
    /// [`Self::discount_applies_to_minimum_charge`] says it does.
    pub fn charge(&self, total_spend_minor: i64) -> EnterpriseSupportCharge {
        let tiered_minor = self.tiered_charge_minor(total_spend_minor);
        let gross_minor = tiered_minor.max(self.minimum_charge_minor);
        let raised_by_minimum = gross_minor > tiered_minor;
        let discountable_minor = if raised_by_minimum && !self.discount_applies_to_minimum_charge {
            0
        } else {
            gross_minor
        };
        let discount_minor = apply_rate(discountable_minor, self.plan_discount_basis_points);
        EnterpriseSupportCharge {
            gross_minor,
            discount_minor,
            total_minor: gross_minor.saturating_sub(discount_minor).max(0),
        }
    }
}

/// An additional charge on an Enterprise Support contract for one billing
/// month, as the modeled `AdditionalCharge` reports it: a charge applied to the
/// contract beside the priced spend, or Support-eligible usage spend added to
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterpriseSupportAdditionalCharge {
    /// The `description` the charge is reported under.
    pub description: String,
    /// The `amount`, in minor units ([`MINOR_UNITS_PER_UNIT`]); `None` is a
    /// charge carrying no amount, which reports no `amount` member.
    pub amount_minor: Option<i64>,
    /// The `chargeType`, when the charge carries one.
    pub charge_type: Option<String>,
}

/// What one billing month adds to the Enterprise Support contract: the instant
/// Support billing was prorated from in that month, and the additional charges
/// and additional Support-eligible usage spend applied to the contract for it.
///
/// Its default is a month with none of them — what a month nothing has been
/// billed for carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnterpriseSupportMonthDetails {
    /// The `supportProrateStartDate`: "the start date for accounts subscribed
    /// or unsubscribed to Support billing during the billing month". `None` is
    /// a month in which no account's subscription changed, so nothing was
    /// prorated and the member is absent.
    pub prorate_start_date: Option<SystemTime>,
    /// The `additionalSupportCharge` entries, in the order they were applied.
    /// Bounded by [`MAX_ADDITIONAL_SUPPORT_CHARGES`].
    pub additional_charges: Vec<EnterpriseSupportAdditionalCharge>,
    /// The `additionalSupportEligibleUsageSpend` entries, in the order they
    /// were applied. Bounded by [`MAX_ADDITIONAL_SUPPORT_CHARGES`].
    pub additional_eligible_usage_spend: Vec<EnterpriseSupportAdditionalCharge>,
}

/// The Enterprise Support contract the Support profile is billed under: how its
/// total charge is distributed across the accounts it covers, and the pricing
/// plans that charge is computed by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterpriseSupportContract {
    /// The `supportAllocationMethod`: [`SUPPORT_ALLOCATION_PROPORTIONAL`] or
    /// [`SUPPORT_ALLOCATION_FIXED_PERCENTAGE`].
    pub allocation_method: String,
    /// The pre-configured share of the total Support charge each account is
    /// charged, in basis points.
    ///
    /// Read only under [`SUPPORT_ALLOCATION_FIXED_PERCENTAGE`]; an account the
    /// contract names no percentage for is charged none of the total.
    pub charge_allocation_basis_points: BTreeMap<String, i64>,
    /// The pricing plans the contract has been billed under. A month is priced
    /// by the one in effect for it ([`Self::effective_pricing_plan`]).
    pub pricing_plans: Vec<EnterpriseSupportPricingPlan>,
    /// The covered accounts whose Support charge is calculated on charges
    /// BEFORE private discounts — the accounts the modeled `ContractAccount`
    /// reports `isGdn` true for ("When true, Support charges are calculated on
    /// charges before private discounts. When false, they are calculated after
    /// private discounts.").
    ///
    /// Empty by default: a covered account's charges are calculated after its
    /// private discounts unless the contract says otherwise.
    pub gdn_accounts: BTreeSet<String>,
    /// The `supportReservedInstanceTreatmentMethod`: how Reserved Instance fees
    /// are included in the Support charge calculation, one of
    /// [`SUPPORT_TREATMENT_METHODS`].
    pub reserved_instance_treatment_method: String,
    /// The custom date Reserved Instance amortization runs from, read only
    /// under [`SUPPORT_TREATMENT_AMORTIZED_CUSTOM`] — the one method the
    /// modeled `supportReservedInstanceAmortizationStartDate` is reported for.
    pub reserved_instance_amortization_start_date: Option<SystemTime>,
    /// The `supportSavingsPlansTreatmentMethod`: how Savings Plans fees are
    /// included in the Support charge calculation, one of
    /// [`SUPPORT_TREATMENT_METHODS`].
    pub savings_plans_treatment_method: String,
    /// The date Savings Plans fees are included in Support-eligible spend from,
    /// read only under [`SUPPORT_TREATMENT_AMORTIZED`] — the one method the
    /// modeled `supportSavingsPlansAmortizationStartDate` is reported for.
    pub savings_plans_amortization_start_date: Option<SystemTime>,
    /// What each billed month adds to the contract, keyed and ordered by the
    /// `YYYY-MM` billing month so one month's detail is reached without
    /// scanning. Bounded by [`MAX_ENTERPRISE_SUPPORT_MONTH_DETAILS`].
    ///
    /// Empty on a fresh emulator, for the reason the spend store is: a month's
    /// proration and additional charges come from a billing run, which this
    /// emulator does not perform.
    pub monthly_details: BTreeMap<String, EnterpriseSupportMonthDetails>,
}

impl Default for EnterpriseSupportContract {
    fn default() -> Self {
        Self {
            // "Proportional means support charges are distributed to each
            // account in proportion to its eligible Spend" — the method that
            // needs no pre-configured percentage, so it is the one a contract
            // nobody has negotiated percentages for is billed under.
            allocation_method: SUPPORT_ALLOCATION_PROPORTIONAL.to_string(),
            charge_allocation_basis_points: BTreeMap::new(),
            pricing_plans: vec![EnterpriseSupportPricingPlan::standard()],
            gdn_accounts: BTreeSet::new(),
            // A stored spend record holds one month's Reserved Instance and
            // Savings Plan spend and the month's charge is computed from the
            // whole of it, which is the `Upfront` treatment of both: no part of
            // a fee is spread over another month.
            reserved_instance_treatment_method: SUPPORT_TREATMENT_UPFRONT.to_string(),
            reserved_instance_amortization_start_date: None,
            savings_plans_treatment_method: SUPPORT_TREATMENT_UPFRONT.to_string(),
            savings_plans_amortization_start_date: None,
            monthly_details: BTreeMap::new(),
        }
    }
}

impl EnterpriseSupportContract {
    /// The plan in effect at `at_secs` seconds past the epoch — the most
    /// recently effective of the plans whose period covers that instant, so a
    /// negotiated plan stored over the standard one prices the months it
    /// covers.
    ///
    /// `None` when no stored plan covers the instant: the profile was not
    /// billed under this contract then, so there is no charge to report for
    /// it.
    pub fn effective_pricing_plan(&self, at_secs: f64) -> Option<&EnterpriseSupportPricingPlan> {
        self.pricing_plans
            .iter()
            .filter(|plan| plan.covers_instant(at_secs))
            .max_by(|left, right| {
                epoch_secs(left.start_date).total_cmp(&epoch_secs(right.start_date))
            })
    }

    /// The plans in effect anywhere in the half-open period
    /// `from_secs..to_secs`, earliest in effect first — the pricing plans the
    /// contract was billed under across that period.
    ///
    /// Empty when no stored plan is in effect in it: the contract was not
    /// billed under any plan then. At most [`MAX_REPORTED_PRICING_PLANS`], the
    /// modeled list bound.
    pub fn pricing_plans_for_period(
        &self,
        from_secs: f64,
        to_secs: f64,
    ) -> Vec<&EnterpriseSupportPricingPlan> {
        let mut plans: Vec<&EnterpriseSupportPricingPlan> = self
            .pricing_plans
            .iter()
            .filter(|plan| plan.covers_period(from_secs, to_secs))
            .collect();
        plans.sort_by(|left, right| {
            epoch_secs(left.start_date).total_cmp(&epoch_secs(right.start_date))
        });
        plans.truncate(MAX_REPORTED_PRICING_PLANS);
        plans
    }

    /// What the contract holds for one `YYYY-MM` billing month, or `None` when
    /// nothing has been billed for that month.
    pub fn month_details(&self, billing_month: &str) -> Option<&EnterpriseSupportMonthDetails> {
        self.monthly_details.get(billing_month)
    }

    /// The month's detail to write, inserting an empty one for a month the
    /// contract holds none for.
    ///
    /// `None` when that would take the store past
    /// [`MAX_ENTERPRISE_SUPPORT_MONTH_DETAILS`], so a writer refuses rather
    /// than growing it without a bound.
    fn month_details_mut(
        &mut self,
        billing_month: &str,
    ) -> Option<&mut EnterpriseSupportMonthDetails> {
        if !self.monthly_details.contains_key(billing_month)
            && self.monthly_details.len() >= MAX_ENTERPRISE_SUPPORT_MONTH_DETAILS
        {
            return None;
        }
        Some(
            self.monthly_details
                .entry(billing_month.to_string())
                .or_default(),
        )
    }

    /// Apply one additional Support charge to the contract for a billing month.
    ///
    /// Answers `false` without writing when the month already holds
    /// [`MAX_ADDITIONAL_SUPPORT_CHARGES`] of them, the modeled list bound, or
    /// when the month detail store is full. The only writer today is this
    /// service's test suite: an additional charge comes from a billing run,
    /// which this emulator does not perform.
    pub fn record_additional_support_charge(
        &mut self,
        billing_month: &str,
        charge: EnterpriseSupportAdditionalCharge,
    ) -> bool {
        let Some(details) = self.month_details_mut(billing_month) else {
            return false;
        };
        if details.additional_charges.len() >= MAX_ADDITIONAL_SUPPORT_CHARGES {
            return false;
        }
        details.additional_charges.push(charge);
        true
    }

    /// Apply one additional Support-eligible usage spend entry to the contract
    /// for a billing month, under the bounds
    /// [`Self::record_additional_support_charge`] writes under.
    pub fn record_additional_support_eligible_usage_spend(
        &mut self,
        billing_month: &str,
        charge: EnterpriseSupportAdditionalCharge,
    ) -> bool {
        let Some(details) = self.month_details_mut(billing_month) else {
            return false;
        };
        if details.additional_eligible_usage_spend.len() >= MAX_ADDITIONAL_SUPPORT_CHARGES {
            return false;
        }
        details.additional_eligible_usage_spend.push(charge);
        true
    }

    /// Record the instant Support billing was prorated from in a billing month
    /// — the month an account subscribed to or unsubscribed from Support
    /// billing in.
    ///
    /// Answers `false` without writing when the month detail store is full.
    pub fn set_support_prorate_start_date(&mut self, billing_month: &str, at: SystemTime) -> bool {
        match self.month_details_mut(billing_month) {
            Some(details) => {
                details.prorate_start_date = Some(at);
                true
            }
            None => false,
        }
    }
}

/// The stored pricing plan as the modeled `PricingPlan` reports it.
///
/// Every amount comes from the plan's own stored amount and every rate from its
/// own stored rate, so the plan a response reports is the one the charge beside
/// it was computed by. The plan models no incremental `increment`, so the two
/// members that describe one are absent rather than invented.
///
/// Shared by every read that projects a pricing plan, so the plan one of them
/// reports and the plan another reports for the same month cannot disagree.
pub fn pricing_plan(plan: &EnterpriseSupportPricingPlan) -> PricingPlan {
    PricingPlan {
        pricing_plan_id: Some(plan.pricing_plan_id.clone()),
        name: Some(plan.name.clone()),
        description: plan.description.clone(),
        start_date: Some(timestamp(plan.start_date)),
        end_date: plan.end_date.map(timestamp),
        plan_discount_percent: Some(rate_decimal(plan.plan_discount_basis_points)),
        discount_applies_to_minimum_charge: Some(plan.discount_applies_to_minimum_charge),
        minimum_charge: Some(amount(plan.minimum_charge_minor)),
        tiered: Some(plan.tiered.to_string()),
        tiers: plan
            .tiers
            .iter()
            .map(|tier| PricingPlanTier {
                tier_minimum: amount(tier.tier_minimum_minor),
                tier_maximum: tier.tier_maximum_minor.map(amount),
                base_charge: amount(tier.base_charge_minor),
                additional_percentage_of_aggregate_charges: rate_decimal(
                    tier.additional_rate_basis_points,
                ),
                aggregate_charges_adjustment: amount(tier.aggregate_charges_adjustment_minor),
                incremental: tier.incremental,
                increment: None,
                increment_charge: None,
            })
            .collect(),
    }
}

/// Stored additional charges as the modeled `AdditionalChargeList` reports
/// them, or `None` when there are none: a month nothing was added to reports no
/// list rather than an empty one.
pub fn additional_charge_list(
    charges: &[EnterpriseSupportAdditionalCharge],
) -> Option<Vec<AdditionalCharge>> {
    if charges.is_empty() {
        return None;
    }
    Some(
        charges
            .iter()
            .take(MAX_ADDITIONAL_SUPPORT_CHARGES)
            .map(|charge| AdditionalCharge {
                description: charge.description.clone(),
                amount: charge.amount_minor.map(amount),
                charge_type: charge.charge_type.clone(),
            })
            .collect(),
    )
}

/// The `AccessDeniedException` payload for a caller that is not the payer
/// account of the Enterprise Support profile.
///
/// The profile's data belongs to the account that pays for the consolidated
/// billing family — `payerAccountId` is "the payer account ID that is
/// authorized to view Enterprise Support data for all accounts in its Support
/// profile" — so every read of that data refuses any other caller, in this one
/// wording, naming the action it asked for.
pub fn enterprise_support_access_denied<Eng: Send + Sync + 'static>(
    operation: &str,
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> AccessDeniedException {
    AccessDeniedException {
        message: format!(
            "User: {} is not authorized to perform: {BILLING_SERVICE_CODE}:{operation} because \
             only the payer account of the Enterprise Support profile may view its Enterprise \
             Support data",
            caller_identity_arn(engine, ctx)
        ),
    }
}

impl ComAmazonawsBillingAwsbillingState {
    /// The share of `total_charge_minor` the Enterprise Support contract
    /// charges `account_id`, given that account's Support-eligible spend and
    /// the whole profile's.
    ///
    /// Under the fixed-percentage method that is the percentage the contract
    /// names for the account; under the proportional method it is the charge in
    /// the proportion the account's spend is of the profile's. A charge cannot
    /// be distributed in proportion to spend when the profile accrued none, so
    /// a profile with no spend charges its whole minimum to the payer account
    /// the profile is billed to.
    pub fn allocated_support_charge_minor(
        &self,
        account_id: &str,
        total_charge_minor: i64,
        account_spend_minor: i64,
        profile_spend_minor: i64,
    ) -> i64 {
        let contract = &self.enterprise_support_contract;
        if contract.allocation_method == SUPPORT_ALLOCATION_FIXED_PERCENTAGE {
            let rate_basis_points = contract
                .charge_allocation_basis_points
                .get(account_id)
                .copied()
                .unwrap_or(0);
            return apply_rate(total_charge_minor, rate_basis_points);
        }
        if profile_spend_minor == 0 {
            return if self.is_billing_family_management_account(account_id) {
                total_charge_minor
            } else {
                0
            };
        }
        prorate(total_charge_minor, account_spend_minor, profile_spend_minor)
    }

    /// The Support-eligible spend records of one billing month that belong to
    /// the Enterprise Support profile `payer_account_id` pays for: the records
    /// of the accounts of its consolidated billing family that were subscribed
    /// to Enterprise Support that month, in ascending account order.
    ///
    /// The family traversal is the one every billing history in this service
    /// reads by
    /// ([`ComAmazonawsBillingAwsbillingState::history_covers_billed_account`]),
    /// and the subscription is the record's own plan: an account on another
    /// Support plan is billed under that plan, not inside this profile.
    pub fn enterprise_support_spend_for_month<'a>(
        &'a self,
        payer_account_id: &'a str,
        billing_month: &str,
    ) -> impl Iterator<Item = &'a SupportEligibleSpend> {
        self.enterprise_support_spend_entries_for_month(payer_account_id, billing_month)
            .map(|(_, spend)| spend)
    }

    /// The same records, each beside the store key it is held under, so a
    /// paginated read of the profile resumes after the account its previous
    /// page ended on instead of counting entries.
    pub fn enterprise_support_spend_entries_for_month<'a>(
        &'a self,
        payer_account_id: &'a str,
        billing_month: &str,
    ) -> impl Iterator<Item = (&'a String, &'a SupportEligibleSpend)> {
        self.support_eligible_spend_for_month(payer_account_id, billing_month)
            .filter(|(_, spend)| spend.support_plan_name == SUPPORT_PLAN_ENTERPRISE)
    }

    /// The accounts the Enterprise Support contract covers, as the modeled
    /// `ContractAccountList` reports them: every account of this emulator's
    /// consolidated billing family, in ascending account order, each carrying
    /// whether its Support charges are calculated before its private discounts
    /// ([`EnterpriseSupportContract::gdn_accounts`]).
    ///
    /// The contract covers the family the Support profile is billed for, so the
    /// covered accounts are a reading of
    /// [`ComAmazonawsBillingAwsbillingState::consolidated_billing_family`] —
    /// the same membership the profile's spend and its charge allocation are
    /// read by, so no two of them can disagree about who the contract covers.
    /// At most [`MAX_CONTRACT_ACCOUNTS`], the modeled list bound.
    pub fn enterprise_support_contract_accounts(&self) -> Vec<ContractAccount> {
        self.consolidated_billing_family
            .keys()
            .take(MAX_CONTRACT_ACCOUNTS)
            .map(|account_id| ContractAccount {
                account_id: account_id.clone(),
                is_gdn: self
                    .enterprise_support_contract
                    .gdn_accounts
                    .contains(account_id),
            })
            .collect()
    }

    /// The payer accounts the Enterprise Support contract charges and the share
    /// of the total Support charge each is pre-configured for, as the modeled
    /// `ChargeAccountList` reports them, in ascending account order.
    ///
    /// The profile's payer account is charged the total however the contract
    /// distributes it, so it is always listed; under
    /// [`SUPPORT_ALLOCATION_FIXED_PERCENTAGE`] every account the contract names
    /// a percentage for is listed beside it, carrying that percentage. Under
    /// [`SUPPORT_ALLOCATION_PROPORTIONAL`] the charge is distributed by each
    /// account's eligible spend rather than by a pre-configured percentage, so
    /// every percentage reported is zero — "This is 0.0 when
    /// supportAllocationMethod = Proportional", the modeled `chargePercentage`
    /// doc. At most [`MAX_CHARGE_ACCOUNTS`], the modeled list bound.
    pub fn enterprise_support_charged_accounts(&self) -> Vec<ChargeAccount> {
        let contract = &self.enterprise_support_contract;
        let mut shares: BTreeMap<&str, i64> = BTreeMap::new();
        shares.insert(self.billing_family_management_account_id.as_str(), 0);
        if contract.allocation_method == SUPPORT_ALLOCATION_FIXED_PERCENTAGE {
            for (account_id, rate_basis_points) in &contract.charge_allocation_basis_points {
                shares.insert(account_id.as_str(), *rate_basis_points);
            }
        }
        shares
            .into_iter()
            .take(MAX_CHARGE_ACCOUNTS)
            .map(|(account_id, rate_basis_points)| ChargeAccount {
                account_id: account_id.to_string(),
                charge_percentage: rate_decimal(rate_basis_points),
            })
            .collect()
    }
}
