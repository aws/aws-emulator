// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-costoptimizationhub-costoptimizationhubservice service.
//!
//! Three stores, all keyed the way Cost Optimization Hub scopes its data:
//!
//! * `enrollments` — one [`Enrollment`] per account id, created with the store
//!   for the account Cost Optimization Hub knows (status `Inactive`, because it
//!   has not opted in), written by `UpdateEnrollmentStatus` — for the calling
//!   account, and for every account of the organization where its management
//!   account asks for its member accounts — and read by
//!   `ListEnrollmentStatuses` — and by the recommendation readers, because Cost
//!   Optimization Hub reports recommendations for an ENROLLED account only, and
//!   only once the analysis that enrollment starts is due.
//! * `preferences` — one [`Preferences`] per account id, written by
//!   `UpdatePreferences` and read by `GetPreferences` (and by the recommendation
//!   readers, because `savingsEstimationMode` decides whether the figures they
//!   report are before or after discounts). A member the account never set reads
//!   back as the default the service applies for it, where it has one —
//!   `memberAccountDiscountVisibility` has none, so it is reported only once it
//!   has been set. That one member is a preference of the ORGANIZATION rather
//!   than of the account, so it lives in the record of the organization's
//!   management account
//!   ([`ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState::discount_visibility_owner`])
//!   and is read through
//!   [`ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState::member_account_discount_visibility`].
//! * `recommendations` + `recommendation_order` — the recommendation catalog
//!   `ListRecommendations` pages over and `GetRecommendation` looks ids up in,
//!   narrowed to what the calling account may read by
//!   [`ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState::readable_recommendation_view`].
//!   The catalog is part of the store's initial value, so both readers see the
//!   same rows and neither has to write anything to serve a read.
//!
//! One fact these reads depend on is deliberately NOT stored here: whether the
//! account is opted into AWS Compute Optimizer, the service whose analysis the
//! efficiency and savings series is derived from. That record belongs to Compute
//! Optimizer, and is read from its store through
//! [`opted_into_compute_optimizer`].

mod compute_optimizer;

pub use compute_optimizer::opted_into_compute_optimizer;

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AuroraDbClusterStorage, AuroraDbClusterStorageConfiguration, BlockStoragePerformanceConfiguration,
    ComputeConfiguration, ComputeSavingsPlans, ComputeSavingsPlansConfiguration, EbsVolume,
    EbsVolumeConfiguration, Ec2Instance, Ec2InstanceConfiguration, EstimatedDiscounts, Filter,
    InstanceConfiguration, LambdaFunction, LambdaFunctionConfiguration, NatGateway,
    NatGatewayConfiguration, PreferredCommitment, ResourceCostCalculation, ResourceDetails,
    ResourcePricing, SavingsPlansCostCalculation, SavingsPlansPricing, StorageConfiguration, Tag,
};

/// The `SavingsEstimationMode` value that subtracts commitment and discount
/// amounts from the figures a recommendation reports; the other value
/// ([`BEFORE_DISCOUNTS`]) reports the undiscounted cost. Both come from the
/// model's `SavingsEstimationMode` enum
/// (`shared_types::SAVINGS_ESTIMATION_MODE_VALUES`).
pub const AFTER_DISCOUNTS: &str = "AfterDiscounts";

/// The other `SavingsEstimationMode` value: the figures are reported before any
/// commitment or discount amount is subtracted.
pub const BEFORE_DISCOUNTS: &str = "BeforeDiscounts";

/// `EnrollmentStatus` values, from the model's enum of the same name.
pub const STATUS_ACTIVE: &str = "Active";

/// The `EnrollmentStatus` values `UpdateEnrollmentStatus` accepts on the wire.
///
/// The PUBLISHED model's enum lists two
/// (`shared_types::ENROLLMENT_STATUS_VALUES`: `Active`, `Inactive`), but the
/// service's own validator admits a third: its recorded rejection of an
/// out-of-enum status reports the accepted set as `[Active, Inactive, Failed]`,
/// `Failed` being a terminal enrollment state the wire enum carries ahead of the
/// published model. The wire set is what a request is judged against
/// (`crate::service::not_in_enum`) and what the refusal of a value outside it
/// advertises.
pub const ENROLLMENT_STATUS_WIRE_VALUES: &[&str] = &["Active", "Inactive", "Failed"];

/// The other `EnrollmentStatus` value: an account Cost Optimization Hub knows
/// about but that has not opted in, which is what an account's enrollment reads
/// as before it ever calls `UpdateEnrollmentStatus`.
pub const STATUS_INACTIVE: &str = "Inactive";

/// How long after an account enrolls its recommendations are due.
///
/// Cost Optimization Hub derives recommendations from the enrolled account's
/// observed usage, so an account that has just opted in has none yet: AWS
/// documents "after you opt in, it can take up to 24 hours for Cost
/// Optimization Hub to display your recommendations". The emulator owes the same
/// transition on an emulator-scale span rather than a real day, so a caller that
/// lists straight after enrolling still observes the pre-analysis state.
const RECOMMENDATION_GENERATION_DELAY: Duration = Duration::from_millis(500);

/// Currency every figure in the catalog is denominated in. Public because a
/// reader that summarises an empty set still reports the unit its zero is in.
pub const CURRENCY_CODE: &str = "USD";

/// An account's enrollment in Cost Optimization Hub.
#[derive(Debug, Clone)]
pub struct Enrollment {
    pub account_id: String,
    /// The account's enrollment status, one of the values the service accepts on
    /// the wire ([`ENROLLMENT_STATUS_WIRE_VALUES`]): `Active` | `Inactive` |
    /// `Failed`.
    pub status: String,
    /// Whether the last `UpdateEnrollmentStatus` asked for the organization's
    /// member accounts to be enrolled as well, or `None` while no
    /// `UpdateEnrollmentStatus` has expressed it — the record Cost Optimization
    /// Hub holds for a known account carries no such preference until the
    /// account enrolls, and `ListEnrollmentStatuses` reports what was stored.
    pub include_member_accounts: Option<bool>,
    pub created_timestamp: DateTime,
    pub last_updated_timestamp: DateTime,
    /// When the analysis this enrollment started is due to have produced
    /// recommendations, or `None` while the account is not enrolled. Set by
    /// `UpdateEnrollmentStatus` when the account becomes `Active`; the instant is
    /// stored, never waited for.
    pub recommendations_due_at: Option<SystemTime>,
}

impl Enrollment {
    /// Whether this enrollment's recommendations are available to read: the
    /// account is enrolled and the analysis the enrollment started is due.
    pub fn recommendations_available(&self) -> bool {
        self.status == STATUS_ACTIVE
            && self
                .recommendations_due_at
                .is_some_and(|due| SystemTime::now() >= due)
    }

    /// The instant an enrollment that becomes `Active` now owes its
    /// recommendations at.
    pub fn generation_due_at() -> SystemTime {
        SystemTime::now() + RECOMMENDATION_GENERATION_DELAY
    }

    /// Move this record to `status`, as of `now`.
    ///
    /// Written once here because one `UpdateEnrollmentStatus` may move SEVERAL
    /// accounts' records (the organization's, where the management account asks
    /// for its member accounts), and every record it moves is moved the same
    /// way: enrolling starts the analysis that produces that account's
    /// recommendations, so the instant they come due is set when the account
    /// becomes `Active` and cleared when it leaves that status — opting in
    /// again starts a fresh analysis, while a call that leaves an already
    /// active account active does not restart it.
    pub fn set_status(&mut self, status: &str, now: DateTime) {
        if status == STATUS_ACTIVE {
            if self.recommendations_due_at.is_none() {
                self.recommendations_due_at = Some(Self::generation_due_at());
            }
        } else {
            self.recommendations_due_at = None;
        }
        self.status = status.to_string();
        // Refreshed on every call that moves the record, including the first.
        self.last_updated_timestamp = now;
    }
}

/// The `SavingsEstimationMode` the service applies to an account that has
/// expressed no preference: the figures are reported BEFORE any discount, which
/// is also the mode `GetPreferences` reports for such an account.
///
/// Recorded behavior: an account that has written no preference is served under
/// before-discounts. The recorded `ListRecommendationSummaries` of such an
/// account reports the `savingsPercentage` metric as unsupported "when savings
/// estimation mode preference is set to before discounts", and a recorded
/// `UpdatePreferences` that sets only another member echoes
/// `savingsEstimationMode` = `BeforeDiscounts` back. The discounted view is the
/// one an account moves itself to with `UpdatePreferences`.
pub const DEFAULT_SAVINGS_ESTIMATION_MODE: &str = BEFORE_DISCOUNTS;

// `memberAccountDiscountVisibility` has NO default the service applies in an
// account's place, so there is no constant for it: the model documents a null
// default for each `PreferredCommitment` member and none for this one, so an
// account that never set it is reported WITHOUT the member — which is what the
// recorded `GetPreferences` body shows (`preferredCommitment` and
// `savingsEstimationMode` only). An account that did set it reads back the
// value it stored, whichever of the enum's two values that is.

/// The `MemberAccountDiscountVisibility` value that makes the organization's
/// member accounts' discounts visible to them; the other value
/// ([`DISCOUNT_VISIBILITY_NONE`]) hides them. Both come from the model's
/// `MemberAccountDiscountVisibility` enum
/// (`shared_types::MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES`).
pub const DISCOUNT_VISIBILITY_ALL: &str = "All";

/// The other `MemberAccountDiscountVisibility` value: the organization's
/// member accounts are shown none of the organization's discounts.
pub const DISCOUNT_VISIBILITY_NONE: &str = "None";

/// The commitment term the service applies where the account states none. From
/// the model's `PreferredCommitment$term` doc: "If the value is null, it will
/// default to `ThreeYears` (highest savings) where applicable".
pub const DEFAULT_COMMITMENT_TERM: &str = "ThreeYears";

/// The payment option the service applies where the account states none. From
/// the model's `PreferredCommitment$paymentOption` doc: "If the value is null,
/// it will default to `AllUpfront` (highest savings) where applicable".
pub const DEFAULT_COMMITMENT_PAYMENT_OPTION: &str = "AllUpfront";

/// An account's Cost Optimization Hub preferences. Every member is optional
/// because an account that has never called `UpdatePreferences` has expressed no
/// preference for it; the value the service then applies — and reports — is the
/// default, which [`Preferences::effective`] resolves.
#[derive(Debug, Clone, Default)]
pub struct Preferences {
    /// `SavingsEstimationMode`: `AfterDiscounts` | `BeforeDiscounts`.
    pub savings_estimation_mode: Option<String>,
    /// `MemberAccountDiscountVisibility`: `All` | `None`.
    pub member_account_discount_visibility: Option<String>,
    pub preferred_commitment: Option<PreferredCommitment>,
}

impl Preferences {
    /// Apply one `UpdatePreferences` request: every member the request supplies
    /// replaces the stored one, and every member it omits keeps its stored value.
    /// The rule applies at each level, so supplying `preferredCommitment.term`
    /// alone leaves the stored `paymentOption` untouched.
    pub fn merge(
        &mut self,
        savings_estimation_mode: Option<String>,
        member_account_discount_visibility: Option<String>,
        preferred_commitment: Option<PreferredCommitment>,
    ) {
        if let Some(mode) = savings_estimation_mode {
            self.savings_estimation_mode = Some(mode);
        }
        if let Some(visibility) = member_account_discount_visibility {
            self.member_account_discount_visibility = Some(visibility);
        }
        if let Some(commitment) = preferred_commitment {
            let mut stored = self.preferred_commitment.take().unwrap_or_default();
            if let Some(term) = commitment.term {
                stored.term = Some(term);
            }
            if let Some(payment_option) = commitment.payment_option {
                stored.payment_option = Some(payment_option);
            }
            self.preferred_commitment = Some(stored);
        }
    }

    /// The account's EFFECTIVE preferences: every member the account set carries
    /// its stored value, and every member it never set carries the default the
    /// service applies in its place, at each level of the record — so a term
    /// stated without a payment option still reports the default payment option.
    /// `memberAccountDiscountVisibility` is the exception: it has no default to
    /// apply (the note beside [`DEFAULT_COMMITMENT_TERM`] says why), so it is
    /// reported only where the account set it — and then with the value that
    /// was stored.
    ///
    /// This is what a read of an account's preferences reports: the preference
    /// the service acts on, never an empty record, because the service always
    /// acts on one.
    pub fn effective(&self) -> Self {
        let commitment = self.preferred_commitment.clone().unwrap_or_default();
        Self {
            savings_estimation_mode: Some(self.effective_savings_estimation_mode().to_string()),
            member_account_discount_visibility: self.member_account_discount_visibility.clone(),
            preferred_commitment: Some(PreferredCommitment {
                term: Some(
                    commitment
                        .term
                        .unwrap_or_else(|| DEFAULT_COMMITMENT_TERM.to_string()),
                ),
                payment_option: Some(
                    commitment
                        .payment_option
                        .unwrap_or_else(|| DEFAULT_COMMITMENT_PAYMENT_OPTION.to_string()),
                ),
            }),
        }
    }

    /// The `savingsEstimationMode` the figures reported to this account are
    /// computed under: the stored preference, or the service's default where the
    /// account expressed none. The recommendation readers resolve the mode
    /// through this, so the money they report agrees with the mode a read of the
    /// preferences reports.
    pub fn effective_savings_estimation_mode(&self) -> &str {
        self.savings_estimation_mode
            .as_deref()
            .unwrap_or(DEFAULT_SAVINGS_ESTIMATION_MODE)
    }
}

/// The monthly price of one resource configuration, split the way
/// `ResourcePricing` models it: the cost before any discount, plus the discount
/// amounts that apply to it.
///
/// The discounts are of two kinds, kept apart because a `savingsEstimationMode`
/// treats them differently: the three itemized amounts are discounts that
/// ALREADY apply to the resource whatever the caller does with the
/// recommendation, while [`Pricing::commitment_purchase_discount`] is the
/// discount adopting the recommendation would itself create.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pricing {
    pub cost_before_discounts: f64,
    pub savings_plans_discount: f64,
    pub reserved_instances_discount: f64,
    pub other_discount: f64,
    /// The savings-plans discount that exists only once the recommended action
    /// is ADOPTED: what the commitment a purchase recommendation recommends
    /// buying takes off the usage it covers. Unlike the discounts above, it is
    /// not one the resource carries today, so it is part of what the recommended
    /// configuration costs under EVERY savings estimation mode
    /// ([`Pricing::cost_for_mode`]).
    pub commitment_purchase_discount: f64,
}

impl Pricing {
    /// A price with no discount applied to it.
    const fn list(cost_before_discounts: f64) -> Self {
        Self {
            cost_before_discounts,
            savings_plans_discount: 0.0,
            reserved_instances_discount: 0.0,
            other_discount: 0.0,
            commitment_purchase_discount: 0.0,
        }
    }

    /// A price with an EXISTING savings-plan discount applied to it: one the
    /// resource carries today, which a discounted view nets out.
    const fn discounted(cost_before_discounts: f64, savings_plans_discount: f64) -> Self {
        Self {
            savings_plans_discount,
            ..Self::list(cost_before_discounts)
        }
    }

    /// The price of a configuration whose savings-plans discount is the one
    /// BUYING the recommended commitment creates: the usage costs
    /// `cost_before_discounts` at public pricing and the commitment takes
    /// `commitment_purchase_discount` off it, so the configuration costs the
    /// difference — the commitment — to whoever adopts the recommendation.
    const fn purchased_commitment(
        cost_before_discounts: f64,
        commitment_purchase_discount: f64,
    ) -> Self {
        Self {
            commitment_purchase_discount,
            ..Self::list(cost_before_discounts)
        }
    }

    /// The discounts the resource carries TODAY, independently of the
    /// recommendation: the ones a discounted view nets out.
    pub fn existing_discounts_total(&self) -> f64 {
        self.savings_plans_discount + self.reserved_instances_discount + self.other_discount
    }

    /// Every discount the price carries, existing or created by adopting the
    /// recommendation.
    pub fn discounts_total(&self) -> f64 {
        self.existing_discounts_total() + self.commitment_purchase_discount
    }

    /// The savings-plans discount to itemize for this price: a discount the
    /// resource carries today and one buying the recommended commitment would
    /// create are both savings-plans discounts, so itemizing them together keeps
    /// the itemized amounts accounting for the whole difference between the cost
    /// before and the cost after discounts.
    pub fn savings_plans_discount_total(&self) -> f64 {
        self.savings_plans_discount + self.commitment_purchase_discount
    }

    pub fn cost_after_discounts(&self) -> f64 {
        round_cents(self.cost_before_discounts - self.discounts_total())
    }

    /// The cost to report under a `savingsEstimationMode`: the configuration's
    /// cost at public pricing, less the discount adopting the recommendation
    /// creates, and — under `AfterDiscounts` only — less the discounts the
    /// resource already carries, which is what `BeforeDiscounts` leaves in.
    ///
    /// The mode selects which EXISTING discounts are netted out of the figures;
    /// it never decides whether the recommended action's own discount counts,
    /// because that discount is the benefit being estimated rather than one the
    /// estimate is taken before or after. The mode is the account's EFFECTIVE
    /// one ([`Preferences::effective_savings_estimation_mode`]), so an account
    /// that has expressed no preference is reported under the service's default.
    pub fn cost_for_mode(&self, mode: Option<&str>) -> f64 {
        let existing = if mode == Some(AFTER_DISCOUNTS) {
            self.existing_discounts_total()
        } else {
            0.0
        };
        round_cents(self.cost_before_discounts - self.commitment_purchase_discount - existing)
    }
}

/// The configuration half of a recommendation's `ResourceDetails`: one variant
/// per resource type in the catalog, carrying the members that resource type's
/// configuration shape declares.
#[derive(Debug, Clone)]
pub enum ResourceConfig {
    Ec2Instance {
        instance_type: String,
    },
    EbsVolume {
        volume_type: String,
        size_in_gb: f64,
        iops: f64,
        throughput: f64,
    },
    LambdaFunction {
        memory_size_in_mb: i32,
        architecture: String,
    },
    ComputeSavingsPlans {
        term: String,
        payment_option: String,
        hourly_commitment: String,
    },
    NatGateway {
        active_connection_count: i64,
    },
    AuroraDbClusterStorage {
        storage_type: String,
    },
}

/// One recommendation in the catalog. `ListRecommendations` renders the list
/// view of it and `GetRecommendation` the detail view, so both answers come from
/// one row and cannot disagree.
#[derive(Debug, Clone)]
pub struct StoredRecommendation {
    pub recommendation_id: String,
    pub account_id: String,
    pub region: String,
    pub resource_id: String,
    pub resource_arn: String,
    /// `ResourceType` values, from the model's enum of the same name.
    pub current_resource_type: String,
    pub recommended_resource_type: String,
    pub current_resource_summary: String,
    pub recommended_resource_summary: String,
    pub current_resource_config: ResourceConfig,
    pub recommended_resource_config: ResourceConfig,
    /// `ActionType` value.
    pub action_type: String,
    /// `ImplementationEffort` value.
    pub implementation_effort: String,
    pub restart_needed: bool,
    pub rollback_possible: bool,
    pub current_pricing: Pricing,
    pub recommended_pricing: Pricing,
    pub recommendation_lookback_period_in_days: i32,
    pub cost_calculation_lookback_period_in_days: i32,
    pub currency_code: String,
    /// `Source` value: `ComputeOptimizer` | `CostExplorer`.
    pub source: String,
    pub last_refresh_timestamp: DateTime,
    pub tags: Vec<(String, String)>,
}

impl StoredRecommendation {
    /// The current resource's monthly cost under `mode`.
    pub fn monthly_cost(&self, mode: Option<&str>) -> f64 {
        self.current_pricing.cost_for_mode(mode)
    }

    /// What adopting the recommendation saves per month under `mode`: the
    /// difference between what the resource costs today and what it would cost
    /// in the recommended configuration — including, for a recommendation whose
    /// action is buying a commitment, the discount that commitment creates,
    /// which is the whole benefit being estimated
    /// ([`Pricing::cost_for_mode`]).
    pub fn monthly_savings(&self, mode: Option<&str>) -> f64 {
        round_cents(
            self.current_pricing.cost_for_mode(mode) - self.recommended_pricing.cost_for_mode(mode),
        )
    }

    /// The monthly saving as a percentage of the current monthly cost.
    pub fn savings_percentage(&self, mode: Option<&str>) -> f64 {
        percentage_of(self.monthly_savings(mode), self.monthly_cost(mode))
    }

    /// The saving over the cost-calculation lookback period: the monthly saving
    /// prorated across that period's length.
    pub fn savings_over_cost_calculation_lookback_period(&self, mode: Option<&str>) -> f64 {
        round_cents(
            self.monthly_savings(mode) * f64::from(self.cost_calculation_lookback_period_in_days)
                / DAYS_PER_MONTH,
        )
    }

    /// The recommendation's tags as the modelled `TagList`, or `None` when it
    /// carries none — the list's `@length` is `1..100`, so an empty list is not
    /// a value this member can take.
    pub fn tag_list(&self) -> Option<Vec<Tag>> {
        if self.tags.is_empty() {
            return None;
        }
        Some(
            self.tags
                .iter()
                .map(|(key, value)| Tag {
                    key: Some(key.clone()),
                    value: Some(value.clone()),
                })
                .collect(),
        )
    }

    pub fn current_resource_details(&self) -> ResourceDetails {
        resource_details(&self.current_resource_config, &self.current_pricing)
    }

    pub fn recommended_resource_details(&self) -> ResourceDetails {
        resource_details(&self.recommended_resource_config, &self.recommended_pricing)
    }

    /// Whether this recommendation satisfies `filter`: every member the filter
    /// sets must match, and a member that lists values matches when the
    /// recommendation's value is one of them.
    pub fn matches_filter(&self, filter: &Filter) -> bool {
        if filter
            .restart_needed
            .is_some_and(|wanted| wanted != self.restart_needed)
        {
            return false;
        }
        if filter
            .rollback_possible
            .is_some_and(|wanted| wanted != self.rollback_possible)
        {
            return false;
        }
        if !matches_any(&filter.recommendation_ids, &self.recommendation_id)
            || !matches_any(&filter.account_ids, &self.account_id)
            || !matches_any(&filter.regions, &self.region)
            || !matches_any(&filter.resource_ids, &self.resource_id)
            || !matches_any(&filter.resource_arns, &self.resource_arn)
            || !matches_any(&filter.action_types, &self.action_type)
            || !matches_any(&filter.implementation_efforts, &self.implementation_effort)
        {
            return false;
        }
        // `resourceTypes` filters on the resource type the recommendation is
        // about, which is the type the resource has today.
        if !matches_any(&filter.resource_types, &self.current_resource_type) {
            return false;
        }
        if filter
            .tags
            .as_ref()
            .is_some_and(|tags| !tags.iter().any(|wanted| self.has_tag(wanted)))
        {
            return false;
        }
        true
    }

    /// Whether the recommendation carries `wanted`: the key must match, and the
    /// value too when the filter states one.
    fn has_tag(&self, wanted: &Tag) -> bool {
        self.tags.iter().any(|(key, value)| {
            wanted.key.as_deref().is_none_or(|k| k == key)
                && wanted.value.as_deref().is_none_or(|v| v == value)
        })
    }

    /// The group label this recommendation falls under for one grouping
    /// dimension: that dimension's value as text, because
    /// `RecommendationSummary.group` is a string whatever type the member it
    /// names carries (`smithy_query query="shape" shape="RecommendationSummary"`).
    pub fn group_label(&self, dimension: Dimension) -> String {
        match dimension {
            Dimension::RestartNeeded => self.restart_needed.to_string(),
            Dimension::RollbackPossible => self.rollback_possible.to_string(),
            other => match self.dimension_key(other) {
                SortKey::Text(text) => text,
                SortKey::Number(number) => number.to_string(),
            },
        }
    }

    /// The value of one `OrderBy` dimension, as a sort key.
    pub fn dimension_key(&self, dimension: Dimension) -> SortKey {
        match dimension {
            Dimension::RecommendationId => SortKey::Text(self.recommendation_id.clone()),
            Dimension::AccountId => SortKey::Text(self.account_id.clone()),
            Dimension::Region => SortKey::Text(self.region.clone()),
            Dimension::ResourceId => SortKey::Text(self.resource_id.clone()),
            Dimension::ResourceArn => SortKey::Text(self.resource_arn.clone()),
            Dimension::CurrentResourceType => SortKey::Text(self.current_resource_type.clone()),
            Dimension::RecommendedResourceType => {
                SortKey::Text(self.recommended_resource_type.clone())
            }
            Dimension::ActionType => SortKey::Text(self.action_type.clone()),
            Dimension::ImplementationEffort => SortKey::Text(self.implementation_effort.clone()),
            Dimension::Source => SortKey::Text(self.source.clone()),
            Dimension::CurrencyCode => SortKey::Text(self.currency_code.clone()),
            Dimension::RestartNeeded => SortKey::Number(f64::from(u8::from(self.restart_needed))),
            Dimension::RollbackPossible => {
                SortKey::Number(f64::from(u8::from(self.rollback_possible)))
            }
            Dimension::RecommendationLookbackPeriodInDays => {
                SortKey::Number(f64::from(self.recommendation_lookback_period_in_days))
            }
            Dimension::LastRefreshTimestamp => {
                SortKey::Number(self.last_refresh_timestamp.as_secs_f64())
            }
        }
    }
}

/// How many days one month of cost covers, for prorating a monthly figure over a
/// lookback period measured in days.
const DAYS_PER_MONTH: f64 = 30.0;

/// A dimension `ListRecommendations`' `orderBy` can sort on. Each one is a
/// `Recommendation` member that carries an ordered value, named as the model
/// names it (`smithy_query query="shape" shape="Recommendation"`); the two
/// mode-dependent money members are not here because the value they carry
/// depends on the caller's stored `savingsEstimationMode`, so
/// [`Dimension::parse`] resolves them in the handler instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    RecommendationId,
    AccountId,
    Region,
    ResourceId,
    ResourceArn,
    CurrentResourceType,
    RecommendedResourceType,
    ActionType,
    ImplementationEffort,
    Source,
    CurrencyCode,
    RestartNeeded,
    RollbackPossible,
    RecommendationLookbackPeriodInDays,
    LastRefreshTimestamp,
}

/// A sort key: a dimension's value in a form two recommendations can be ordered
/// by.
#[derive(Debug, Clone, PartialEq)]
pub enum SortKey {
    Text(String),
    Number(f64),
}

impl SortKey {
    /// Order two keys of the same dimension. Keys of different kinds never meet:
    /// one dimension always yields one kind.
    pub fn compare(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
            (Self::Number(a), Self::Number(b)) => a.total_cmp(b),
            (Self::Text(_), Self::Number(_)) => std::cmp::Ordering::Less,
            (Self::Number(_), Self::Text(_)) => std::cmp::Ordering::Greater,
        }
    }
}

/// The money dimensions whose value depends on the stored
/// `savingsEstimationMode`, resolved against a recommendation by the handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyDimension {
    EstimatedMonthlyCost,
    EstimatedMonthlySavings,
    EstimatedSavingsPercentage,
}

/// Either kind of `orderBy` dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderDimension {
    Plain(Dimension),
    Money(MoneyDimension),
}

impl Dimension {
    /// Resolve an `orderBy.dimension` value, or `None` when it names no
    /// `Recommendation` member. Compared case-insensitively, so `accountId` and
    /// `AccountId` name the same member.
    pub fn parse(dimension: &str) -> Option<OrderDimension> {
        let key = dimension.trim().to_ascii_lowercase();
        let plain = match key.as_str() {
            "recommendationid" => Self::RecommendationId,
            "accountid" => Self::AccountId,
            "region" => Self::Region,
            "resourceid" => Self::ResourceId,
            "resourcearn" => Self::ResourceArn,
            "currentresourcetype" => Self::CurrentResourceType,
            "recommendedresourcetype" => Self::RecommendedResourceType,
            "actiontype" => Self::ActionType,
            "implementationeffort" => Self::ImplementationEffort,
            "source" => Self::Source,
            "currencycode" => Self::CurrencyCode,
            "restartneeded" => Self::RestartNeeded,
            "rollbackpossible" => Self::RollbackPossible,
            "recommendationlookbackperiodindays" => Self::RecommendationLookbackPeriodInDays,
            "lastrefreshtimestamp" => Self::LastRefreshTimestamp,
            "estimatedmonthlycost" => {
                return Some(OrderDimension::Money(MoneyDimension::EstimatedMonthlyCost))
            }
            "estimatedmonthlysavings" => {
                return Some(OrderDimension::Money(
                    MoneyDimension::EstimatedMonthlySavings,
                ))
            }
            "estimatedsavingspercentage" => {
                return Some(OrderDimension::Money(
                    MoneyDimension::EstimatedSavingsPercentage,
                ))
            }
            _ => return None,
        };
        Some(OrderDimension::Plain(plain))
    }
}

/// The dimensions a recommendation summary can be grouped by: the CATEGORICAL
/// recommendation dimensions, whose values form a set a caller can be shown one
/// total per.
///
/// The per-resource identifiers (`recommendationId`, `resourceId`,
/// `resourceArn`) are not here — `ListRecommendationSummaries` documents those
/// three as unsupported for this API, and one group per resource is no summary —
/// and neither are the continuous members (the money figures, the lookback
/// period and the refresh timestamp), which carry a quantity rather than a
/// category.
pub const GROUP_DIMENSIONS: &[Dimension] = &[
    Dimension::AccountId,
    Dimension::Region,
    Dimension::CurrentResourceType,
    Dimension::RecommendedResourceType,
    Dimension::ActionType,
    Dimension::ImplementationEffort,
    Dimension::Source,
    Dimension::CurrencyCode,
    Dimension::RestartNeeded,
    Dimension::RollbackPossible,
];

/// Resolve a `groupBy` value, or `None` when it names no dimension a summary can
/// be grouped by.
///
/// The dimension is named as the model names the `Recommendation` member
/// (case-insensitively, as `orderBy` is), plus `ResourceType`: the name
/// `Filter.resourceTypes` gives the type the resource has today, which is the
/// type the recommendation is about.
pub fn parse_group_dimension(group_by: &str) -> Option<Dimension> {
    if group_by.trim().eq_ignore_ascii_case("ResourceType") {
        return Some(Dimension::CurrentResourceType);
    }
    match Dimension::parse(group_by)? {
        OrderDimension::Plain(dimension) if GROUP_DIMENSIONS.contains(&dimension) => Some(dimension),
        _ => None,
    }
}

/// Keep one recommendation per resource: the one that saves the most under
/// `mode`, because that is the recommendation for the resource. Ties keep the
/// catalog's order, so the choice is stable across calls.
///
/// This is the de-duplication both recommendation views need: the list view
/// serves it when `includeAllRecommendations` is false, and the summary view
/// totals it into `estimatedTotalDedupedSavings`.
pub fn dedupe_by_resource(
    rows: &[StoredRecommendation],
    mode: Option<&str>,
) -> Vec<StoredRecommendation> {
    let mut best: Vec<StoredRecommendation> = Vec::with_capacity(rows.len());
    for row in rows {
        match best
            .iter()
            .position(|kept| kept.resource_id == row.resource_id)
        {
            None => best.push(row.clone()),
            Some(index) => {
                if row.monthly_savings(mode) > best[index].monthly_savings(mode) {
                    best[index] = row.clone();
                }
            }
        }
    }
    best
}

/// Whether `value` is in `allowed`, treating an absent list as "not filtered".
fn matches_any(allowed: &Option<Vec<String>>, value: &str) -> bool {
    allowed
        .as_ref()
        .is_none_or(|values| values.iter().any(|v| v == value))
}

/// Round a money amount to whole cents, so a figure derived by subtraction does
/// not report binary-float noise.
pub fn round_cents(amount: f64) -> f64 {
    (amount * 100.0).round() / 100.0
}

/// `part` as a percentage of `whole`, to two decimal places; zero when `whole`
/// is zero, because no percentage of nothing is defined.
pub fn percentage_of(part: f64, whole: f64) -> f64 {
    if whole == 0.0 {
        return 0.0;
    }
    round_cents(part / whole * 100.0)
}

/// Build the modelled `ResourceDetails` union for one resource configuration and
/// its price.
fn resource_details(config: &ResourceConfig, pricing: &Pricing) -> ResourceDetails {
    match config {
        ResourceConfig::Ec2Instance { instance_type } => {
            ResourceDetails::ec2Instance(Ec2Instance {
                configuration: Some(Ec2InstanceConfiguration {
                    instance: Some(InstanceConfiguration {
                        r#type: Some(instance_type.clone()),
                    }),
                }),
                cost_calculation: Some(resource_cost_calculation(pricing)),
            })
        }
        ResourceConfig::EbsVolume {
            volume_type,
            size_in_gb,
            iops,
            throughput,
        } => ResourceDetails::ebsVolume(EbsVolume {
            configuration: Some(EbsVolumeConfiguration {
                storage: Some(StorageConfiguration {
                    r#type: Some(volume_type.clone()),
                    size_in_gb: Some(*size_in_gb),
                }),
                performance: Some(BlockStoragePerformanceConfiguration {
                    iops: Some(*iops),
                    throughput: Some(*throughput),
                }),
                attachment_state: Some("attached".to_string()),
            }),
            cost_calculation: Some(resource_cost_calculation(pricing)),
        }),
        ResourceConfig::LambdaFunction {
            memory_size_in_mb,
            architecture,
        } => ResourceDetails::lambdaFunction(LambdaFunction {
            configuration: Some(LambdaFunctionConfiguration {
                compute: Some(ComputeConfiguration {
                    v_cpu: None,
                    memory_size_in_mb: Some(*memory_size_in_mb),
                    architecture: Some(architecture.clone()),
                    platform: None,
                }),
            }),
            cost_calculation: Some(resource_cost_calculation(pricing)),
        }),
        ResourceConfig::ComputeSavingsPlans {
            term,
            payment_option,
            hourly_commitment,
        } => ResourceDetails::computeSavingsPlans(ComputeSavingsPlans {
            configuration: Some(ComputeSavingsPlansConfiguration {
                account_scope: Some("Payer".to_string()),
                term: Some(term.clone()),
                payment_option: Some(payment_option.clone()),
                hourly_commitment: Some(hourly_commitment.clone()),
            }),
            cost_calculation: Some(SavingsPlansCostCalculation {
                pricing: Some(SavingsPlansPricing {
                    monthly_savings_plans_eligible_cost: Some(round_cents(
                        pricing.cost_before_discounts,
                    )),
                    estimated_monthly_commitment: Some(pricing.cost_after_discounts()),
                    savings_percentage: Some(percentage_of(
                        pricing.discounts_total(),
                        pricing.cost_before_discounts,
                    )),
                    estimated_on_demand_cost: Some(round_cents(pricing.cost_before_discounts)),
                }),
            }),
        }),
        ResourceConfig::NatGateway {
            active_connection_count,
        } => ResourceDetails::natGateway(NatGateway {
            configuration: Some(NatGatewayConfiguration {
                active_connection_count: Some(*active_connection_count),
                packets_in_from_source: None,
                packets_in_from_destination: None,
            }),
            cost_calculation: Some(resource_cost_calculation(pricing)),
        }),
        ResourceConfig::AuroraDbClusterStorage { storage_type } => {
            ResourceDetails::auroraDbClusterStorage(AuroraDbClusterStorage {
                configuration: Some(AuroraDbClusterStorageConfiguration {
                    storage_type: Some(storage_type.clone()),
                }),
                cost_calculation: Some(resource_cost_calculation(pricing)),
            })
        }
    }
}

/// The `ResourceCostCalculation` for a price: the modelled split of the cost
/// before discounts, the discounts, and the cost after them.
fn resource_cost_calculation(pricing: &Pricing) -> ResourceCostCalculation {
    ResourceCostCalculation {
        usages: None,
        pricing: Some(ResourcePricing {
            estimated_cost_before_discounts: Some(round_cents(pricing.cost_before_discounts)),
            estimated_net_unused_amortized_commitments: None,
            estimated_discounts: Some(EstimatedDiscounts {
                savings_plans_discount: Some(round_cents(pricing.savings_plans_discount_total())),
                reserved_instances_discount: Some(round_cents(pricing.reserved_instances_discount)),
                other_discount: Some(round_cents(pricing.other_discount)),
            }),
            estimated_cost_after_discounts: Some(pricing.cost_after_discounts()),
        }),
    }
}

/// The account whose Cost Optimization Hub data a request addresses.
///
/// Every generated emulator presents the one canonical account id
/// ([`engine::emulator_account_id`]); it is never derived from the caller's
/// access key.
pub fn caller_account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// Version marker inside every pagination token this service issues, so a token
/// from an older encoding is refused rather than misread.
const PAGE_TOKEN_VERSION: &str = "cohv1";

/// Encode the offset a page ends at into an opaque `nextToken`.
///
/// `kind` names the list the token belongs to, so a token issued by one
/// paginated operation is not accepted by another.
pub fn encode_page_token(kind: &str, offset: usize) -> String {
    ::hex::encode(format!("{PAGE_TOKEN_VERSION}:{kind}:{offset}"))
}

/// The offset a `nextToken` carries, or `None` when the token is not in the
/// encoding [`encode_page_token`] writes for this `kind`.
///
/// Whether the offset is one this service ever handed out for a list is decided
/// against that list, by [`paginate`], which is the whole rule and the only
/// caller a handler reaches this through.
fn decode_page_token(kind: &str, token: &str) -> Option<usize> {
    let decoded = ::hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let offset = text.strip_prefix(&format!("{PAGE_TOKEN_VERSION}:{kind}:"))?;
    offset.parse::<usize>().ok()
}

/// One page of a list: the rows to return and the token that resumes after them.
#[derive(Debug)]
pub struct Page<'a, T> {
    pub items: &'a [T],
    pub next_token: Option<String>,
}

/// That a `nextToken` is not one this service issued for the list being read, so
/// it names no row to resume at: either it is not in the encoding
/// [`encode_page_token`] writes for that list, or it carries an offset no page of
/// that list ever ended at.
#[derive(Debug, PartialEq, Eq)]
pub struct TokenNotIssued;

/// The row a `nextToken` for a list of `len` rows resumes at, or
/// [`TokenNotIssued`] when the list never handed that token out.
///
/// [`paginate`] issues a token only when rows remain after the page it closes,
/// and it closes a page carrying at least one row, so every offset this service
/// hands out for a list of `len` rows points at a row of that list and never at
/// its first: the issued offsets are `1..len`. An offset outside them resumes at
/// no row the list could have resumed at, so it was not issued for it.
fn issued_offset(kind: &str, len: usize, token: &str) -> Result<usize, TokenNotIssued> {
    let offset = decode_page_token(kind, token).ok_or(TokenNotIssued)?;
    if offset == 0 || offset >= len {
        return Err(TokenNotIssued);
    }
    Ok(offset)
}

/// Cut the page `next_token` asks for out of `all`, or refuse the token.
///
/// No token asks for the first row; a token this list issued resumes at the row
/// the page that issued it stopped at; any other value is [`TokenNotIssued`],
/// because it names no row of this list to resume at and serving a page in its
/// place would answer an invalid request as a listing.
///
/// `max_results` caps the page; absent, the whole remainder is returned. The
/// token is issued only when rows remain after this page, so the last page
/// carries none.
pub fn paginate<'a, T>(
    kind: &str,
    all: &'a [T],
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<Page<'a, T>, TokenNotIssued> {
    let start = match next_token {
        None => 0,
        Some(token) => issued_offset(kind, all.len(), token)?,
    };
    let remaining = &all[start..];
    let limit = max_results.map_or(remaining.len(), |m| usize::try_from(m).unwrap_or(0));
    let items = &remaining[..limit.min(remaining.len())];
    let end = start + items.len();
    let next_token = if items.is_empty() || end >= all.len() {
        None
    } else {
        Some(encode_page_token(kind, end))
    };
    Ok(Page { items, next_token })
}

/// Per-service state for com-amazonaws-costoptimizationhub-costoptimizationhubservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState {
    /// Enrollment per account id: the record Cost Optimization Hub holds for
    /// every account it knows, created with the store and updated by
    /// `UpdateEnrollmentStatus`.
    pub enrollments: HashMap<String, Enrollment>,
    /// Preferences per account id, written by `UpdatePreferences`.
    pub preferences: HashMap<String, Preferences>,
    /// The recommendation catalog, by recommendation id.
    pub recommendations: HashMap<String, StoredRecommendation>,
    /// The catalog's order, so listing and paging it are stable across calls.
    pub recommendation_order: Vec<String>,
    /// The account that is the MANAGEMENT account of the organization this
    /// store records, if any.
    ///
    /// Cost Optimization Hub scopes two different things to that account, and
    /// they are asked as two different questions:
    ///
    /// * reading the enrollment or the recommendations of accounts OTHER than
    ///   the caller's is the organization's view, which only the management
    ///   account of an organization with member accounts has
    ///   ([`Self::is_management_account`]). This emulator presents one account
    ///   ([`caller_account_id`]) and records no member accounts of its own, so
    ///   the field holds `None` and a read reaching past the caller is refused
    ///   for want of an organization to read;
    /// * the organization-wide member-account discount-visibility preference
    ///   belongs to the management account of the organization it is about
    ///   ([`Self::owns_member_account_discount_visibility`]), the account that
    ///   PAYS for its member accounts' spend. Where the field holds `None` the
    ///   preference belongs to no account at all: an account that manages no
    ///   organization has no member accounts whose discounts it could show or
    ///   hide, so it neither sets the preference nor is reported it.
    pub organization_management_account_id: Option<String>,
}

impl ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState {
    /// Whether `account_id` is the management account of the organization this
    /// store records. False whenever no management account is recorded: there
    /// is then no organization whose member accounts' data a read could reach,
    /// which is what this answer gates. Who owns the organization-wide
    /// discount-visibility preference is a different question, asked of
    /// [`Self::owns_member_account_discount_visibility`].
    pub fn is_management_account(&self, account_id: &str) -> bool {
        self.organization_management_account_id.as_deref() == Some(account_id)
    }

    /// The account the organization-wide member-account discount-visibility
    /// preference belongs to: the management account of the organization the
    /// preference is about, which is the account that pays for the member
    /// accounts' spend the discounts are on.
    ///
    /// `None` where this store records no organization — the state every client
    /// of this emulator meets, the account it presents managing no organization
    /// of its own. The preference is then NO account's: with no member accounts
    /// to show discounts to, there is nothing for it to decide, so no caller
    /// sets it and no read reports it.
    pub fn discount_visibility_owner(&self) -> Option<&str> {
        self.organization_management_account_id.as_deref()
    }

    /// Whether `account_id` is the account the organization-wide member-account
    /// discount-visibility preference belongs to — the only account that may
    /// set it, and the only one a read reports it to
    /// ([`Self::discount_visibility_owner`] says which account that is).
    pub fn owns_member_account_discount_visibility(&self, account_id: &str) -> bool {
        self.discount_visibility_owner() == Some(account_id)
    }

    /// The organization-wide member-account discount-visibility preference in
    /// effect, or `None` where it has never been set.
    ///
    /// The preference is one setting of the ORGANIZATION, not one per account,
    /// so it is the record of the account that owns it
    /// ([`Self::discount_visibility_owner`]) that holds it; any other account's
    /// stored value is not the preference in effect.
    pub fn member_account_discount_visibility(&self) -> Option<&str> {
        self.preferences
            .get(self.discount_visibility_owner()?)?
            .member_account_discount_visibility
            .as_deref()
    }

    /// Whether the organization's member accounts are shown the discounts their
    /// spend carries: the organization-wide preference above set to
    /// [`DISCOUNT_VISIBILITY_ALL`].
    ///
    /// This is the prerequisite of the discount-adjusted savings estimation
    /// mode ([`AFTER_DISCOUNTS`]): the figures that mode reports are net of
    /// those discounts, so they can be reported only where the organization has
    /// made them visible.
    pub fn member_account_discounts_visible(&self) -> bool {
        self.member_account_discount_visibility() == Some(DISCOUNT_VISIBILITY_ALL)
    }
}

impl Default for ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState {
    /// The store's initial value: the caller's enrollment record (the account
    /// is known to Cost Optimization Hub, and has not opted in), no preference
    /// (nobody has called `UpdatePreferences` yet), no organization, and the
    /// recommendation catalog this emulator reports.
    ///
    /// The catalog is part of the initial value rather than something a reader
    /// writes on first use, so `ListRecommendations` and `GetRecommendation`
    /// serve the same rows without either of them mutating the store. The
    /// enrollment record is there for the same reason: `ListEnrollmentStatuses`
    /// reports an account's status whether or not it has ever enrolled, so the
    /// record cannot be something the first write creates.
    fn default() -> Self {
        let account_id = caller_account_id().to_string();
        let created = DateTime::from(std::time::SystemTime::now());
        let mut state = Self {
            enrollments: HashMap::from([(
                account_id.clone(),
                Enrollment {
                    account_id: account_id.clone(),
                    status: STATUS_INACTIVE.to_string(),
                    include_member_accounts: None,
                    created_timestamp: created,
                    last_updated_timestamp: created,
                    // The account has not opted in, so no analysis has been
                    // started and no recommendations are owed.
                    recommendations_due_at: None,
                },
            )]),
            preferences: HashMap::new(),
            recommendations: HashMap::new(),
            recommendation_order: Vec::new(),
            organization_management_account_id: None,
        };
        for recommendation in seed_recommendations() {
            state
                .recommendation_order
                .push(recommendation.recommendation_id.clone());
            state
                .recommendations
                .insert(recommendation.recommendation_id.clone(), recommendation);
        }
        state
    }
}

impl ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState {
    /// The account's effective preferences: what it set, with the service's
    /// default for every member it never set (see [`Preferences::effective`]).
    /// An account that has never called `UpdatePreferences` has a record of
    /// defaults, not an empty one.
    ///
    /// `memberAccountDiscountVisibility` is the organization's own preference
    /// rather than this account's, so it is reported to the account that owns it
    /// alone ([`Self::owns_member_account_discount_visibility`]) — the account
    /// whose record holds it; every other account is reported the record without
    /// that member.
    pub fn effective_preferences(&self, account_id: &str) -> Preferences {
        let mut effective = self
            .preferences
            .get(account_id)
            .cloned()
            .unwrap_or_default()
            .effective();
        if !self.owns_member_account_discount_visibility(account_id) {
            effective.member_account_discount_visibility = None;
        }
        effective
    }

    /// The accounts one read of this service's data addresses.
    ///
    /// A request that NAMES accounts (`ListEnrollmentStatuses`' `accountId`,
    /// the recommendation readers' `filter.accountIds`) addresses exactly those
    /// accounts; naming none it addresses the calling account, widened to every
    /// account of the organization where the request asks for the
    /// organization's view (`includeOrganizationInfo`). The precedence between
    /// those members lives in [`ReadScope`], so the accounts resolved here and
    /// the question a permission gate asks of the same request
    /// ([`Self::addressed_accounts_for_read`]) are answers about one scope.
    ///
    /// The result is sorted and carries each account once, so a paginated
    /// reader resumes at the row the previous page stopped at.
    pub fn addressed_accounts(
        &self,
        caller: &str,
        named: &[String],
        whole_organization: bool,
    ) -> Vec<String> {
        self.accounts_in(ReadScope::of(named, whole_organization), caller)
    }

    /// The accounts a read addresses, or the refusal the caller's permissions
    /// earn it: a read whose scope reaches past the caller's own account is the
    /// organization's view, which only the organization's management account
    /// has, so any other caller is refused ([`OrganizationViewRequired`], which
    /// the reader maps into the modelled error its own operation declares).
    ///
    /// Gate and scoping are decided from the ONE [`ReadScope`] the request
    /// resolves to, so a request admitted as reaching no further than the
    /// caller's own record cannot then be scoped to another account's, and a
    /// request whose scope is confined to the caller's own record cannot be
    /// refused as an organization view.
    pub fn addressed_accounts_for_read(
        &self,
        caller: &str,
        named: &[String],
        whole_organization: bool,
    ) -> Result<Vec<String>, OrganizationViewRequired> {
        let scope = ReadScope::of(named, whole_organization);
        if scope.reaches_past(caller) && !self.is_management_account(caller) {
            return Err(OrganizationViewRequired);
        }
        Ok(self.accounts_in(scope, caller))
    }

    /// The accounts `scope` resolves to, sorted and each carried once.
    fn accounts_in(&self, scope: ReadScope<'_>, caller: &str) -> Vec<String> {
        let mut accounts: Vec<String> = match scope {
            ReadScope::Named(named) => named.to_vec(),
            ReadScope::WholeOrganization => self.enrollments.keys().cloned().collect(),
            ReadScope::CallerOnly => vec![caller.to_string()],
        };
        accounts.sort();
        accounts.dedup();
        accounts
    }

    /// The view a read of ONE recommendation resolves its id over: every
    /// recommendation this caller may read, in catalog order, whichever account
    /// owns it, under the caller's effective savings estimation mode — the same
    /// [`RecommendationView`] the list readers are served, because such a read
    /// names no account and sets no filter.
    ///
    /// A read that names no account addresses the caller's own account, widened
    /// to the organization where the caller is its management account — the
    /// caller whose reads past its own account are admitted at all, and so the
    /// caller whose `filter.accountIds` a list reader serves another account's
    /// rows to ([`Self::recommendation_view`]). The rows are therefore the union
    /// of what this caller's list reads can select, and an id a list handed it
    /// reads back.
    ///
    /// Enrollment is still judged per owning account
    /// ([`Self::visible_recommendations_for`]), so an account that never opted
    /// in, one that opted out, and one that has just opted in hold no readable
    /// recommendation at all.
    pub fn readable_recommendation_view(&self, account_id: &str) -> RecommendationView {
        let addressed =
            self.addressed_accounts(account_id, &[], self.is_management_account(account_id));
        self.view_over(account_id, &addressed, None)
    }

    /// The recommendations `accounts` can be read for, in catalog order.
    ///
    /// Cost Optimization Hub reports recommendations for an ENROLLED account
    /// only, and only once the analysis that enrollment starts is due (see
    /// [`RECOMMENDATION_GENERATION_DELAY`]), so an addressed account that never
    /// opted in, one that opted out, and one whose analysis is not due yet each
    /// contribute nothing rather than someone else's catalog. A read may
    /// address more than one account (a management account naming members in
    /// `filter.accountIds`), which is why the set is judged per account here.
    pub fn visible_recommendations_for(&self, accounts: &[String]) -> Vec<StoredRecommendation> {
        let readable: Vec<&str> = accounts
            .iter()
            .filter(|account| {
                self.enrollments
                    .get(account.as_str())
                    .is_some_and(Enrollment::recommendations_available)
            })
            .map(String::as_str)
            .collect();
        self.recommendation_order
            .iter()
            .filter_map(|id| self.recommendations.get(id))
            .filter(|row| readable.contains(&row.account_id.as_str()))
            .cloned()
            .collect()
    }

    /// The recommendation rows one request selects, and the savings estimation
    /// mode their money members are computed under.
    ///
    /// The one place the rule both LIST readers apply is written: a read whose
    /// scope reaches PAST the caller's own account — a `filter.accountIds`
    /// naming any other account — is an organization view, which only the
    /// organization's management account has, so any other caller is refused
    /// ([`OrganizationViewRequired`], which each reader answers through its own
    /// modelled error), while a scope confined to the caller's own account is
    /// that account's own record and is served whoever asks. That question is
    /// the enrollment reader's too, so it is asked once, of the one
    /// [`ReadScope`] the request resolves to
    /// ([`Self::addressed_accounts_for_read`]).
    ///
    /// Otherwise the rows are the ones the accounts the request ADDRESSES may
    /// be read for (`view_over`) narrowed by every member the filter sets,
    /// under the account's effective `savingsEstimationMode` — the view the
    /// reader of ONE recommendation is served too
    /// ([`Self::readable_recommendation_view`]).
    pub fn recommendation_view(
        &self,
        account_id: &str,
        filter: Option<&Filter>,
    ) -> Result<RecommendationView, OrganizationViewRequired> {
        let named = filter
            .and_then(|filter| filter.account_ids.as_deref())
            .unwrap_or_default();
        // Gate and scoping from the one scope: the read is refused only where
        // it reaches an account other than the caller's, and the rows then come
        // from the accounts it addresses — the ones it named, else the caller's
        // own — so a read admitted as reaching another account answers that
        // account's rows rather than an empty list.
        let addressed = self.addressed_accounts_for_read(account_id, named, false)?;
        Ok(self.view_over(account_id, &addressed, filter))
    }

    /// The view every recommendation reader is served, given the accounts that
    /// reader's own rule addresses: the rows those accounts may be read for
    /// ([`Self::visible_recommendations_for`]) narrowed by every member the
    /// filter sets, reported under the caller's effective
    /// `savingsEstimationMode` ([`Self::effective_preferences`]).
    ///
    /// Written once here so the mode a reader reports its money members under,
    /// and the narrowing it applies to the rows it selects, cannot be changed
    /// for one reader and left behind in another: which accounts a reader
    /// addresses is the only thing that differs between them, and it is what
    /// each passes in.
    fn view_over(
        &self,
        account_id: &str,
        accounts: &[String],
        filter: Option<&Filter>,
    ) -> RecommendationView {
        RecommendationView {
            savings_estimation_mode: self
                .effective_preferences(account_id)
                .savings_estimation_mode,
            rows: self
                .visible_recommendations_for(accounts)
                .into_iter()
                .filter(|row| filter.is_none_or(|filter| row.matches_filter(filter)))
                .collect(),
        }
    }
}

/// What the one view-building step of
/// [`ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState`] hands a
/// recommendation reader: the rows the request selects and the mode the money
/// members of those rows are reported under.
#[derive(Debug)]
pub struct RecommendationView {
    /// The caller's effective `savingsEstimationMode` preference, which decides
    /// whether the figures are reported before or after discounts.
    pub savings_estimation_mode: Option<String>,
    /// The rows this caller may read, narrowed by the request's filter, in
    /// catalog order.
    pub rows: Vec<StoredRecommendation>,
}

/// What one read of this service's data is scoped to, before the accounts it
/// resolves to are looked up in the store.
///
/// A request that NAMES accounts is scoped to exactly those; naming none it is
/// scoped to the whole organization where it asks for the organization's view,
/// else to the calling account alone. That precedence — a named account beats
/// the organization's view, so naming the caller's own account is a read of
/// that one record however the organization member is set — is written once
/// here, because both the accounts a reader scopes its rows to
/// ([`ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState::addressed_accounts`])
/// and whether its caller needs the organization's view at all
/// ([`ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState::addressed_accounts_for_read`])
/// are questions about this one scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadScope<'a> {
    /// Exactly the accounts the request named.
    Named(&'a [String]),
    /// Every account of the organization the store records.
    WholeOrganization,
    /// The calling account alone.
    CallerOnly,
}

impl<'a> ReadScope<'a> {
    /// The scope a request's account-naming and organization-view members
    /// resolve to.
    fn of(named: &'a [String], whole_organization: bool) -> Self {
        if !named.is_empty() {
            Self::Named(named)
        } else if whole_organization {
            Self::WholeOrganization
        } else {
            Self::CallerOnly
        }
    }

    /// Whether this scope reaches any account other than the caller's: a named
    /// scope only when it names one, the organization's view always (it is the
    /// organization's member accounts that view is for, whichever of them the
    /// store happens to hold a record for), and the caller's own scope never.
    fn reaches_past(&self, caller: &str) -> bool {
        match self {
            Self::Named(named) => named.iter().any(|wanted| wanted != caller),
            Self::WholeOrganization => true,
            Self::CallerOnly => false,
        }
    }
}

/// The refusal a read of accounts other than the caller's earns a caller that
/// is not the organization's management account, and so has no organization
/// view
/// ([`ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState::addressed_accounts_for_read`],
/// [`ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState::recommendation_view`]).
/// Each reader maps it into the modelled error its own operation declares for
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrganizationViewRequired;

/// The recommendation catalog, generated once when the store is created.
///
/// The emulator observes no real usage, so the catalog is a fixed set of
/// resources rather than something derived from traffic; every figure a
/// recommendation reports is nevertheless computed from the prices stored here
/// (so cost, saving and percentage agree with each other and with the
/// `ResourceDetails` cost calculation), and both recommendation readers read
/// these rows.
fn seed_recommendations() -> Vec<StoredRecommendation> {
    let account_id = caller_account_id().to_string();
    let now = DateTime::from(std::time::SystemTime::now());
    let east = "us-east-1";
    let west = "us-west-2";

    vec![
        // Two recommendations for ONE instance: rightsizing it, or moving it to
        // Graviton. `includeAllRecommendations` decides whether a caller sees
        // both or only the better one.
        StoredRecommendation {
            recommendation_id: "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0001".to_string(),
            account_id: account_id.clone(),
            region: east.to_string(),
            resource_id: "i-0a1b2c3d4e5f60001".to_string(),
            resource_arn: ::engine::emulator_arn("ec2", east, "instance/i-0a1b2c3d4e5f60001"),
            current_resource_type: "Ec2Instance".to_string(),
            recommended_resource_type: "Ec2Instance".to_string(),
            current_resource_summary: "m5.2xlarge".to_string(),
            recommended_resource_summary: "m5.xlarge".to_string(),
            current_resource_config: ResourceConfig::Ec2Instance {
                instance_type: "m5.2xlarge".to_string(),
            },
            recommended_resource_config: ResourceConfig::Ec2Instance {
                instance_type: "m5.xlarge".to_string(),
            },
            action_type: "Rightsize".to_string(),
            implementation_effort: "VeryLow".to_string(),
            restart_needed: true,
            rollback_possible: true,
            current_pricing: Pricing::discounted(280.32, 28.03),
            recommended_pricing: Pricing::discounted(140.16, 14.02),
            recommendation_lookback_period_in_days: 14,
            cost_calculation_lookback_period_in_days: 30,
            currency_code: CURRENCY_CODE.to_string(),
            source: "ComputeOptimizer".to_string(),
            last_refresh_timestamp: now,
            tags: vec![
                ("environment".to_string(), "production".to_string()),
                ("team".to_string(), "payments".to_string()),
            ],
        },
        StoredRecommendation {
            recommendation_id: "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0002".to_string(),
            account_id: account_id.clone(),
            region: east.to_string(),
            resource_id: "i-0a1b2c3d4e5f60001".to_string(),
            resource_arn: ::engine::emulator_arn("ec2", east, "instance/i-0a1b2c3d4e5f60001"),
            current_resource_type: "Ec2Instance".to_string(),
            recommended_resource_type: "Ec2Instance".to_string(),
            current_resource_summary: "m5.2xlarge".to_string(),
            recommended_resource_summary: "m6g.2xlarge".to_string(),
            current_resource_config: ResourceConfig::Ec2Instance {
                instance_type: "m5.2xlarge".to_string(),
            },
            recommended_resource_config: ResourceConfig::Ec2Instance {
                instance_type: "m6g.2xlarge".to_string(),
            },
            action_type: "MigrateToGraviton".to_string(),
            implementation_effort: "Medium".to_string(),
            restart_needed: true,
            rollback_possible: true,
            current_pricing: Pricing::discounted(280.32, 28.03),
            recommended_pricing: Pricing::discounted(224.26, 22.43),
            recommendation_lookback_period_in_days: 14,
            cost_calculation_lookback_period_in_days: 30,
            currency_code: CURRENCY_CODE.to_string(),
            source: "ComputeOptimizer".to_string(),
            last_refresh_timestamp: now,
            tags: vec![
                ("environment".to_string(), "production".to_string()),
                ("team".to_string(), "payments".to_string()),
            ],
        },
        StoredRecommendation {
            recommendation_id: "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0003".to_string(),
            account_id: account_id.clone(),
            region: east.to_string(),
            resource_id: "vol-00112233445566003".to_string(),
            resource_arn: ::engine::emulator_arn("ec2", east, "volume/vol-00112233445566003"),
            current_resource_type: "EbsVolume".to_string(),
            recommended_resource_type: "EbsVolume".to_string(),
            current_resource_summary: "gp2 500 GiB".to_string(),
            recommended_resource_summary: "gp3 500 GiB".to_string(),
            current_resource_config: ResourceConfig::EbsVolume {
                volume_type: "gp2".to_string(),
                size_in_gb: 500.0,
                iops: 1500.0,
                throughput: 128.0,
            },
            recommended_resource_config: ResourceConfig::EbsVolume {
                volume_type: "gp3".to_string(),
                size_in_gb: 500.0,
                iops: 3000.0,
                throughput: 125.0,
            },
            action_type: "Upgrade".to_string(),
            implementation_effort: "Low".to_string(),
            restart_needed: false,
            rollback_possible: true,
            current_pricing: Pricing::list(50.0),
            recommended_pricing: Pricing::list(40.0),
            recommendation_lookback_period_in_days: 14,
            cost_calculation_lookback_period_in_days: 30,
            currency_code: CURRENCY_CODE.to_string(),
            source: "ComputeOptimizer".to_string(),
            last_refresh_timestamp: now,
            tags: vec![("environment".to_string(), "production".to_string())],
        },
        StoredRecommendation {
            recommendation_id: "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0004".to_string(),
            account_id: account_id.clone(),
            region: west.to_string(),
            resource_id: "invoice-renderer".to_string(),
            resource_arn: ::engine::emulator_arn("lambda", west, "function:invoice-renderer"),
            current_resource_type: "LambdaFunction".to_string(),
            recommended_resource_type: "LambdaFunction".to_string(),
            current_resource_summary: "3008 MB, x86_64".to_string(),
            recommended_resource_summary: "1024 MB, x86_64".to_string(),
            current_resource_config: ResourceConfig::LambdaFunction {
                memory_size_in_mb: 3008,
                architecture: "x86_64".to_string(),
            },
            recommended_resource_config: ResourceConfig::LambdaFunction {
                memory_size_in_mb: 1024,
                architecture: "x86_64".to_string(),
            },
            action_type: "Rightsize".to_string(),
            implementation_effort: "Low".to_string(),
            restart_needed: false,
            rollback_possible: true,
            current_pricing: Pricing::list(72.4),
            recommended_pricing: Pricing::list(24.8),
            recommendation_lookback_period_in_days: 30,
            cost_calculation_lookback_period_in_days: 30,
            currency_code: CURRENCY_CODE.to_string(),
            source: "ComputeOptimizer".to_string(),
            last_refresh_timestamp: now,
            tags: vec![("environment".to_string(), "staging".to_string())],
        },
        StoredRecommendation {
            recommendation_id: "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0005".to_string(),
            account_id: account_id.clone(),
            region: east.to_string(),
            resource_id: "compute-savings-plan-recommendation".to_string(),
            resource_arn: ::engine::emulator_arn(
                "savingsplans",
                "",
                "savingsplan/compute-savings-plan-recommendation",
            ),
            current_resource_type: "ComputeSavingsPlans".to_string(),
            recommended_resource_type: "ComputeSavingsPlans".to_string(),
            current_resource_summary: "No commitment".to_string(),
            recommended_resource_summary: "ThreeYears, AllUpfront, 1.20 USD/hour".to_string(),
            current_resource_config: ResourceConfig::ComputeSavingsPlans {
                term: "ThreeYears".to_string(),
                payment_option: "AllUpfront".to_string(),
                hourly_commitment: "0.00".to_string(),
            },
            recommended_resource_config: ResourceConfig::ComputeSavingsPlans {
                term: "ThreeYears".to_string(),
                payment_option: "AllUpfront".to_string(),
                hourly_commitment: "1.20".to_string(),
            },
            action_type: "PurchaseSavingsPlans".to_string(),
            implementation_effort: "VeryLow".to_string(),
            restart_needed: false,
            rollback_possible: false,
            // Buying the recommended commitment is what creates this row's
            // discount: the eligible usage costs 1_314.00 a month on demand and
            // the commitment covers it for 876.00, so adopting the
            // recommendation saves the difference under every savings
            // estimation mode.
            current_pricing: Pricing::list(1_314.0),
            recommended_pricing: Pricing::purchased_commitment(1_314.0, 438.0),
            recommendation_lookback_period_in_days: 60,
            cost_calculation_lookback_period_in_days: 60,
            currency_code: CURRENCY_CODE.to_string(),
            source: "CostExplorer".to_string(),
            last_refresh_timestamp: now,
            tags: Vec::new(),
        },
        StoredRecommendation {
            recommendation_id: "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0006".to_string(),
            account_id: account_id.clone(),
            region: west.to_string(),
            resource_id: "nat-0f1e2d3c4b5a60006".to_string(),
            resource_arn: ::engine::emulator_arn("ec2", west, "natgateway/nat-0f1e2d3c4b5a60006"),
            current_resource_type: "NatGateway".to_string(),
            recommended_resource_type: "NatGateway".to_string(),
            current_resource_summary: "Idle NAT gateway".to_string(),
            recommended_resource_summary: "Delete".to_string(),
            current_resource_config: ResourceConfig::NatGateway {
                active_connection_count: 0,
            },
            recommended_resource_config: ResourceConfig::NatGateway {
                active_connection_count: 0,
            },
            action_type: "Delete".to_string(),
            implementation_effort: "High".to_string(),
            restart_needed: true,
            rollback_possible: false,
            current_pricing: Pricing::list(32.85),
            recommended_pricing: Pricing::list(0.0),
            recommendation_lookback_period_in_days: 14,
            cost_calculation_lookback_period_in_days: 14,
            currency_code: CURRENCY_CODE.to_string(),
            source: "CostExplorer".to_string(),
            last_refresh_timestamp: now,
            tags: vec![("environment".to_string(), "sandbox".to_string())],
        },
        StoredRecommendation {
            recommendation_id: "0ec38a3a-1a0a-4a4f-9d2a-1f1e9a7c0007".to_string(),
            account_id,
            region: west.to_string(),
            resource_id: "ledger-aurora-cluster".to_string(),
            resource_arn: ::engine::emulator_arn("rds", west, "cluster:ledger-aurora-cluster"),
            current_resource_type: "AuroraDbClusterStorage".to_string(),
            recommended_resource_type: "AuroraDbClusterStorage".to_string(),
            current_resource_summary: "aurora".to_string(),
            recommended_resource_summary: "aurora-iopt1".to_string(),
            current_resource_config: ResourceConfig::AuroraDbClusterStorage {
                storage_type: "aurora".to_string(),
            },
            recommended_resource_config: ResourceConfig::AuroraDbClusterStorage {
                storage_type: "aurora-iopt1".to_string(),
            },
            action_type: "Upgrade".to_string(),
            implementation_effort: "VeryHigh".to_string(),
            restart_needed: false,
            rollback_possible: true,
            current_pricing: Pricing::list(910.5),
            recommended_pricing: Pricing::list(728.4),
            recommendation_lookback_period_in_days: 30,
            cost_calculation_lookback_period_in_days: 30,
            currency_code: CURRENCY_CODE.to_string(),
            source: "CostExplorer".to_string(),
            last_refresh_timestamp: now,
            tags: vec![("team".to_string(), "ledger".to_string())],
        },
    ]
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState>)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a stored `savingsEstimationMode` does to a row's money members:
    /// `AfterDiscounts` reports the cost net of every discount the resource
    /// already carries, and every other state of the preference —
    /// `BeforeDiscounts`, or no preference at all, which is served under the
    /// same mode ([`DEFAULT_SAVINGS_ESTIMATION_MODE`]) — reports the cost with
    /// those discounts left in. Asserted on the stored prices directly; the
    /// wire behaviour of each mode is `tests/all/cost_optimization_hub.rs`.
    #[test]
    fn the_mode_decides_whether_discounts_are_subtracted() {
        let pricing = Pricing {
            cost_before_discounts: 280.32,
            savings_plans_discount: 20.0,
            reserved_instances_discount: 5.0,
            other_discount: 3.03,
            commitment_purchase_discount: 0.0,
        };

        // (the stored mode, the cost it reports)
        let cases: [(Option<&str>, f64); 4] = [
            (None, 280.32),
            (Some(BEFORE_DISCOUNTS), 280.32),
            (Some(AFTER_DISCOUNTS), 252.29),
            // A value outside the enum cannot reach the store, and an
            // unrecognised one is not `AfterDiscounts`: the undiscounted cost.
            (Some("Nonsense"), 280.32),
        ];
        for (mode, expected) in cases {
            let actual = pricing.cost_for_mode(mode);
            assert!(
                (actual - expected).abs() < 0.005,
                "case {mode:?}: expected cost {expected}, got {actual}"
            );
        }
    }

    /// One row of the table in
    /// [`a_discount_the_recommendation_creates_counts_under_every_mode`]: the
    /// case, the current and the recommended price, and what each state of the
    /// preference reports as `(mode, monthly cost, monthly saving)`.
    type SavingsCase = (
        &'static str,
        Pricing,
        Pricing,
        [(Option<&'static str>, f64, f64); 3],
    );

    /// The discount adopting a recommendation would CREATE is part of what the
    /// recommended configuration costs under every savings estimation mode, so a
    /// recommendation whose whole benefit is that discount saves the same amount
    /// in every mode, while the discounts a resource already carries are netted
    /// out only by the discounted mode.
    ///
    /// The table spans what the rule generalises over: whether the recommended
    /// configuration's discount is one the resource already carries or one
    /// buying a commitment creates (or both at once), whether the current
    /// resource carries a discount of its own, and each state of the preference.
    #[test]
    fn a_discount_the_recommendation_creates_counts_under_every_mode() {
        let cases: [SavingsCase; 4] = [
            (
                "a commitment purchase: the recommended configuration is the \
                 commitment, and nothing is discounted today",
                Pricing::list(1_200.0),
                Pricing::purchased_commitment(1_200.0, 400.0),
                [
                    (None, 1_200.0, 400.0),
                    (Some(BEFORE_DISCOUNTS), 1_200.0, 400.0),
                    (Some(AFTER_DISCOUNTS), 1_200.0, 400.0),
                ],
            ),
            (
                "a commitment purchase on usage that already carries a discount",
                Pricing::discounted(1_200.0, 120.0),
                Pricing {
                    savings_plans_discount: 120.0,
                    ..Pricing::purchased_commitment(1_200.0, 400.0)
                },
                [
                    // Before discounts the existing 120.00 is left in on both
                    // sides and cancels; after discounts it is netted out of
                    // both, so either way the saving is the commitment's.
                    (None, 1_200.0, 400.0),
                    (Some(BEFORE_DISCOUNTS), 1_200.0, 400.0),
                    (Some(AFTER_DISCOUNTS), 1_080.0, 400.0),
                ],
            ),
            (
                "a rightsizing whose both sides carry an existing discount",
                Pricing::discounted(280.32, 28.03),
                Pricing::discounted(140.16, 14.02),
                [
                    (None, 280.32, 140.16),
                    (Some(BEFORE_DISCOUNTS), 280.32, 140.16),
                    (Some(AFTER_DISCOUNTS), 252.29, 126.15),
                ],
            ),
            (
                "an undiscounted upgrade",
                Pricing::list(50.0),
                Pricing::list(40.0),
                [
                    (None, 50.0, 10.0),
                    (Some(BEFORE_DISCOUNTS), 50.0, 10.0),
                    (Some(AFTER_DISCOUNTS), 50.0, 10.0),
                ],
            ),
        ];

        for (case, current, recommended, expectations) in cases {
            for (mode, expected_cost, expected_savings) in expectations {
                let cost = current.cost_for_mode(mode);
                let savings = round_cents(cost - recommended.cost_for_mode(mode));
                assert!(
                    (cost - expected_cost).abs() < 0.005,
                    "case {case:?} {mode:?}: expected cost {expected_cost}, got {cost}"
                );
                assert!(
                    (savings - expected_savings).abs() < 0.005,
                    "case {case:?} {mode:?}: expected saving {expected_savings}, got {savings}"
                );
            }
            // Whatever the mode, the itemized discounts still account for the
            // whole difference between the cost before and after discounts.
            for pricing in [current, recommended] {
                let itemized = pricing.savings_plans_discount_total()
                    + pricing.reserved_instances_discount
                    + pricing.other_discount;
                let after = round_cents(pricing.cost_before_discounts - itemized);
                assert!(
                    (after - pricing.cost_after_discounts()).abs() < 0.005,
                    "case {case:?}: expected the itemized discounts to leave {}, got {after}",
                    pricing.cost_after_discounts()
                );
            }
        }
    }

    /// The store starts with the caller's enrollment record (`Inactive`, no
    /// `includeMemberAccounts` preference) and with no organization, so no
    /// account is the management account.
    #[test]
    fn the_initial_store_knows_the_caller_and_no_organization() {
        let state = ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState::default();
        let caller = caller_account_id();
        let enrollment = state
            .enrollments
            .get(caller)
            .expect("the caller's enrollment record");
        assert_eq!(
            enrollment.status, STATUS_INACTIVE,
            "expected an account that has not opted in, got {:?}",
            enrollment.status
        );
        assert_eq!(
            enrollment.include_member_accounts, None,
            "expected no includeMemberAccounts preference, got {:?}",
            enrollment.include_member_accounts
        );
        assert_eq!(
            state.enrollments.len(),
            1,
            "expected only the caller's record, got {:?}",
            state.enrollments.keys().collect::<Vec<_>>()
        );
        for account in [caller, "111122223333", ""] {
            assert!(
                !state.is_management_account(account),
                "case {account:?}: no account is the management account of an \
                 organization this emulator does not record"
            );
            assert!(
                !state.owns_member_account_discount_visibility(account),
                "case {account:?}: the organization-wide preference belongs to no \
                 account while no organization is recorded"
            );
        }
        assert_eq!(
            state.discount_visibility_owner(),
            None,
            "expected no owner of the organization-wide preference, got {:?}",
            state.discount_visibility_owner()
        );
    }

    /// The organization-wide `memberAccountDiscountVisibility` preference is
    /// read, and reported, for the MANAGEMENT account of the recorded
    /// organization alone: a record that holds the member for any other
    /// account — the caller's own included — is not reported it, and the
    /// preference in effect, which is what the discount-adjusted savings
    /// estimation mode is gated on, is the management account's. While no
    /// organization is recorded the preference is no account's at all.
    #[test]
    fn the_discount_visibility_preference_belongs_to_the_management_account() {
        let member = "111122223333";
        let management = "444455556666";
        let mut state = ComAmazonawsCostoptimizationhubCostoptimizationhubserviceState::default();
        for account in [member, management] {
            state.preferences.insert(
                account.to_string(),
                Preferences {
                    savings_estimation_mode: None,
                    member_account_discount_visibility: Some(
                        DISCOUNT_VISIBILITY_ALL.to_string(),
                    ),
                    preferred_commitment: None,
                },
            );
        }

        // No organization recorded: the preference is no account's, so no
        // record's value is reported or in effect — not even the caller's.
        let caller = caller_account_id();
        state
            .preferences
            .entry(caller.to_string())
            .or_default()
            .member_account_discount_visibility = Some(DISCOUNT_VISIBILITY_ALL.to_string());
        for account in [member, management, caller] {
            assert_eq!(
                state
                    .effective_preferences(account)
                    .member_account_discount_visibility,
                None,
                "case no organization, account {account:?}: expected the member to be \
                 left out of the report"
            );
        }
        assert!(
            !state.member_account_discounts_visible(),
            "case no organization: expected no preference in effect, got {:?}",
            state.member_account_discount_visibility()
        );

        // With the caller recorded as the organization's management account,
        // its own record is the one that holds the preference, and its value
        // decides whether the discounts are visible.
        state.organization_management_account_id = Some(caller.to_string());
        for value in [DISCOUNT_VISIBILITY_ALL, DISCOUNT_VISIBILITY_NONE] {
            state
                .preferences
                .entry(caller.to_string())
                .or_default()
                .member_account_discount_visibility = Some(value.to_string());
            assert_eq!(
                state
                    .effective_preferences(caller)
                    .member_account_discount_visibility
                    .as_deref(),
                Some(value),
                "case the caller manages the organization and set {value:?}: expected the \
                 preference reported back"
            );
            assert_eq!(
                state.member_account_discounts_visible(),
                value == DISCOUNT_VISIBILITY_ALL,
                "case the caller manages the organization and set {value:?}: expected the \
                 visibility to follow the value, got {:?}",
                state.member_account_discount_visibility()
            );
        }

        // With another account recorded as the management account, ITS record
        // is the one that holds the preference — and the caller's own stored
        // value is no longer reported to it, because the organization is not
        // the caller's.
        state.organization_management_account_id = Some(management.to_string());
        assert_eq!(
            state
                .effective_preferences(caller)
                .member_account_discount_visibility,
            None,
            "case another account manages the organization: expected the member to be left \
             out of the caller's report"
        );
        assert_eq!(
            state
                .effective_preferences(management)
                .member_account_discount_visibility
                .as_deref(),
            Some(DISCOUNT_VISIBILITY_ALL),
            "case management account: expected the preference reported"
        );
        assert_eq!(
            state
                .effective_preferences(member)
                .member_account_discount_visibility,
            None,
            "case member account: expected the member to be left out of the report"
        );
        assert!(
            state.member_account_discounts_visible(),
            "case management account set All: expected the discounts visible"
        );

        // And the value decides: the other value of the enum is not visibility.
        state
            .preferences
            .get_mut(management)
            .expect("the management account's record")
            .member_account_discount_visibility = Some(DISCOUNT_VISIBILITY_NONE.to_string());
        assert!(
            !state.member_account_discounts_visible(),
            "case management account set None: expected the discounts not visible"
        );
    }
}
