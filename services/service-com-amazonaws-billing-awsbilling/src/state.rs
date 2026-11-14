// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-billing-awsbilling service.
//!
//! The service's primary resource is the **billing view**. Every one of the five
//! implemented lifecycle operations (Create/Get/List/Update/DeleteBillingView)
//! computes its answer from [`ComAmazonawsBillingAwsbillingState::billing_views`],
//! which is keyed by the view's ARN and ordered, so a list and its pagination are
//! deterministic.
//!
//! Its second resource is the account's **billing preferences**:
//! `UpdateBillingPreferences` writes [`ComAmazonawsBillingAwsbillingState::billing_preferences`]
//! (and appends to [`ComAmazonawsBillingAwsbillingState::preference_history`] for a
//! feature the model gives a history feature), and `GetBillingPreferences` reports
//! exactly what is stored there. Both maps are ordered, so a page boundary is stable.
//! Which features either operation may touch at all is
//! [`ComAmazonawsBillingAwsbillingState::feature_access`], the per-feature access
//! gate both consult before they read or write a preference.
//!
//! Beside them, [`ComAmazonawsBillingAwsbillingState::credits`] holds the account
//! credits `GetCredits` reports, keyed and ordered by `creditId`, and
//! [`ComAmazonawsBillingAwsbillingState::credit_allocations`] the credits
//! applied to an account's bills, keyed by [`CreditAllocation::key`] so a
//! reverse walk is the `billingMonth`-descending order
//! `GetCreditAllocationHistory` documents.
//!
//! [`ComAmazonawsBillingAwsbillingState::support_eligible_spend`] holds the
//! Support-eligible spend of each linked account and billing month, from which
//! `ListBusinessSupportAccountCharges` PRICES the Support charges it reports —
//! see the `support_charges` module below. The same spend is what
//! `GetEnterpriseSupportChargeSummary` prices for the accounts of the
//! Enterprise Support profile, by the pricing plan of
//! [`ComAmazonawsBillingAwsbillingState::enterprise_support_contract`] — see
//! the `enterprise_support` module. The money and billing-month arithmetic both
//! reports share lives in the `money` and `billing_month` modules, and the
//! Support subscription contracts those same bills record —
//! what `ListBusinessSupportSubscriptionHistory` reports — are read off them by
//! the `support_subscriptions` module.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;

use crate::ops::types::{
    AccessDeniedException, BillingViewHealthStatus, BillingViewHealthStatusException, Expression,
    ResourceTag, ValidationException, ValidationExceptionField, BILLING_FEATURE_VALUES,
};

/// The Support-eligible spend `ListBusinessSupportAccountCharges` prices.
mod support_charges;

pub use support_charges::*;

/// The Support subscription contracts those same bills record, which
/// `ListBusinessSupportSubscriptionHistory` reports.
mod support_subscriptions;

pub use support_subscriptions::*;

/// The `YYYY-MM` billing-month arithmetic the monthly charge reports share.
mod billing_month;

pub use billing_month::*;

/// The Enterprise Support contract `GetEnterpriseSupportChargeSummary` prices
/// the profile's spend by.
mod enterprise_support;

pub use enterprise_support::*;

/// The money and rate arithmetic the charge reports share.
mod money;

pub use money::*;

/// When each account of the billing family was linked and subscribed to
/// Enterprise Support, which `ListEnterpriseSupportLinkedAccountCharges`
/// prorates an account's spend by.
mod linked_accounts;

pub use linked_accounts::*;

/// `CUSTOM`, the `BillingViewType` a view created through `CreateBillingView`
/// carries (one of `BILLING_VIEW_TYPE_VALUES` in `ops::types`).
pub const BILLING_VIEW_TYPE_CUSTOM: &str = "CUSTOM";

/// `PRIMARY`, the `BillingViewType` of the view every account owns by default.
///
/// From the `ListBillingViews` doc: "Every Amazon Web Services account has a
/// unique `PRIMARY` billing view that represents the billing data available by
/// default."
pub const BILLING_VIEW_TYPE_PRIMARY: &str = "PRIMARY";

/// `HEALTHY`, a `BillingViewStatus` value (`BILLING_VIEW_STATUS_VALUES`).
pub const BILLING_VIEW_STATUS_HEALTHY: &str = "HEALTHY";

/// `STARTS_WITH`, the only `SearchOption` the model declares, used by the
/// `ListBillingViews` `names` filter.
pub const SEARCH_OPTION_STARTS_WITH: &str = "STARTS_WITH";

/// The `resourceType` the modeled error shapes report for a billing view.
pub const BILLING_VIEW_RESOURCE_TYPE: &str = "billingview";

/// The most billing views this emulator keeps resident.
///
/// `billing_views` grows with every `CreateBillingView` call, so it needs a
/// ceiling: without one a caller's request rate is the only limit on the memory
/// of a process that also serves every other emulated service. This is an
/// emulator resource bound, not a published AWS quota; `CreateBillingView`
/// refuses past it with the modeled `ServiceQuotaExceededException` ("You've
/// reached the limit of resources you can create").
pub const MAX_BILLING_VIEWS: usize = 256;

/// The most `clientToken` -> ARN entries this emulator keeps resident.
///
/// An entry is written only beside a view it names and removed when that view is
/// deleted, so the map stays no larger than `billing_views`; the cap is checked
/// anyway, so a future caller of the store cannot grow it past a known bound.
pub const MAX_IDEMPOTENCY_TOKENS: usize = MAX_BILLING_VIEWS;

/// The `serviceCode` the modeled `ServiceQuotaExceededException` reports.
pub const BILLING_SERVICE_CODE: &str = "billing";

/// The `quotaCode` the modeled `ServiceQuotaExceededException` reports for the
/// billing-view count.
pub const BILLING_VIEWS_QUOTA_CODE: &str = "BillingViewsPerAccount";

/// The most source views one billing view can draw from.
///
/// This is the model's own bound, not an emulator choice: both
/// `AssociateSourceViews$sourceViews` and `ListSourceViewsForBillingView`'s
/// output target `BillingViewSourceViewsList`, whose `@length` is 1..10. A
/// request that would push a view past it is refused with the modeled
/// `ServiceQuotaExceededException` ("exceeded the size of an individual
/// resource").
pub const MAX_SOURCE_VIEWS_PER_VIEW: usize = 10;

/// The `quotaCode` the modeled `ServiceQuotaExceededException` reports for the
/// per-view source-view count.
///
/// Synthesised in the same shape as [`BILLING_VIEWS_QUOTA_CODE`]: the real
/// service's quota code for this limit is not published or modelled, so a later
/// fix can replace the string without re-deriving anything else here.
pub const SOURCE_VIEWS_QUOTA_CODE: &str = "SourceViewsPerBillingView";

/// The most tags one resource holds.
///
/// `TagResource` merges a request's tags into the resource's stored set, so
/// without a ceiling a caller's request rate would be the only limit on the
/// memory of a process that also serves every other emulated service — the same
/// reason [`MAX_BILLING_VIEWS`] exists. 200 is the figure the model itself
/// names: `ResourceTagList` is `@length` 0..200. That modelled length bounds ONE
/// request's list; this bounds what a resource ends up holding across requests,
/// and `TagResource` refuses past it with the modeled `ValidationException`.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The `ValidationExceptionReason` a refusal that names the offending member
/// carries (one of `unknownOperation | cannotParse | fieldValidationFailed |
/// other`).
pub const VALIDATION_REASON_FIELD: &str = "fieldValidationFailed";

/// What a refusal puts where it would otherwise echo the value that was
/// rejected.
///
/// This service states a constraint violation WITHOUT the submitted value —
/// several of its constrained members are `@sensitive` and none of them belongs
/// in a 400 body — so where the sentence has a place for the value, that place
/// carries this marker instead. See `crate::service`, which renders the
/// refusals.
pub const REDACTED_VALUE: &str = "[redacted]";

/// The `ValidationExceptionReason` a refusal that is about the caller rather
/// than about one request member carries.
pub const VALIDATION_REASON_OTHER: &str = "other";

/// The `ValidationExceptionReason` a refusal of a value this service cannot
/// read at all carries — a `nextToken` it did not issue, say.
pub const VALIDATION_REASON_CANNOT_PARSE: &str = "cannotParse";

/// The `message` every paginated read refuses an unusable `nextToken` with.
pub const PAGE_TOKEN_REFUSAL: &str = "nextToken is not a token this service issued";

/// The `ValidationExceptionField` message that accompanies
/// [`PAGE_TOKEN_REFUSAL`].
pub const PAGE_TOKEN_FIELD_REFUSAL: &str = "The pagination token is not valid.";

/// The `message` the tagging APIs refuse an unknown resource with.
///
/// The tagging operations identify a resource by `ResourceArn` and answer a
/// FIXED sentence for one that names no resource: the refusal does not echo the
/// requested ARN back, unlike the billing-view reads, whose refusal is built by
/// [`invalid_billing_view_arn`]. The ARN is still reported where the model has a
/// member for it, `ResourceNotFoundException$resourceId`.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The `message` that refuses a tagging request against a view AWS manages.
///
/// Shared by the tagging operations so the one rule — a managed view's tags are
/// the service's own, see [`BillingView::is_managed`] — reads the same whichever
/// of them answered.
pub const MANAGED_VIEW_NOT_TAGGABLE: &str = "Managed billing views cannot be tagged";

/// The most credits this emulator keeps resident for the whole billing family.
///
/// `credits` is the store `GetCredits` reports from; like `billing_views` it
/// needs a ceiling so that nothing can grow the process without bound. This is
/// an emulator resource bound, not a published AWS quota:
/// [`ComAmazonawsBillingAwsbillingState::insert_credit`] refuses past it.
pub const MAX_CREDITS: usize = 1024;

/// The resource id of the account's default `PRIMARY` billing view.
const PRIMARY_VIEW_ID: &str = "primary";

/// The name AWS gives the account's default `PRIMARY` billing view.
const PRIMARY_VIEW_NAME: &str = "Primary View";

/// The fixed first half of the description the account's default `PRIMARY`
/// billing view carries; [`primary_view_description`] completes it.
const PRIMARY_VIEW_DESCRIPTION_PREFIX: &str = "Primary billing view for accountId: ";

/// The description the service itself writes for the default `PRIMARY` billing
/// view of the account `owner_account_id` owns.
///
/// `BillingViewElement$description` is "The description of the billing view",
/// and the view every account owns by default is a resource the service seeded,
/// so it arrives WITH one. The value is not a constant: it names the owning
/// account, so two accounts' default views describe themselves differently —
/// [`PRIMARY_VIEW_DESCRIPTION_PREFIX`] followed by the id of the account that
/// owns the view.
///
/// The modelled `BillingViewDescription` pattern (`^([ a-zA-Z0-9_\+=\.\-@]+)?$`)
/// admits no colon, so this value is outside it. That pattern constrains what a
/// CALLER may send — `CreateBillingView` and `UpdateBillingView` are still
/// refused outside it at the request boundary — and the description of a
/// service-written resource is what the surface answers with, not caller input.
pub fn primary_view_description(owner_account_id: &str) -> String {
    format!("{PRIMARY_VIEW_DESCRIPTION_PREFIX}{owner_account_id}")
}

/// The `BillingFeature` values whose preferences a caller reaches only once
/// Billing-console access is activated for the account.
///
/// The access decision is per FEATURE, not per operation: the recorded service
/// refuses `BILLING_ALERTS` and `RI_SHARING_HISTORY` reads and
/// `RI_SHARING`/`RI_SHARING_HISTORY` writes with 403 `AccessDeniedException`
/// for a caller without that activation, while a credit feature
/// (`CREDIT_SHARING`) answers normally for the same caller. These are the
/// features behind the activation — the billing-alerts preference and the
/// Reserved-Instance sharing pair, whose history feature travels with the
/// feature whose changes it records (see [`HISTORY_FEATURE_SUFFIX`]); every
/// other `BILLING_FEATURE_VALUES` member is reachable without it.
///
/// What an ACCOUNT has activated is state, not a constant: this is only the
/// default, which
/// [`ComAmazonawsBillingAwsbillingState::feature_access`] is seeded from and
/// [`ComAmazonawsBillingAwsbillingState::grant_feature_access`] moves.
pub const ACCESS_ACTIVATION_FEATURES: &[&str] =
    &["BILLING_ALERTS", "RI_SHARING", "RI_SHARING_HISTORY"];

/// The `BillingFeature` values whose preferences are *sharing* preferences.
///
/// From the `UpdateBillingPreferences` doc: "Sharing keys (`RI_SHARING`,
/// `CREDIT_SHARING`, `CREDIT_LEVEL_SHARING`, and sharing keys under
/// `CREDIT_PREFERENCE_OPTIONS`) may only be set by the management account of a
/// consolidated billing family."
pub const SHARING_FEATURES: &[&str] = &["RI_SHARING", "CREDIT_SHARING", "CREDIT_LEVEL_SHARING"];

/// `CREDIT_PREFERENCE_OPTIONS`, the one `BillingFeature` whose keys name a single
/// credit (`credit/{creditId}/status`, from the `UpdateBillingPreferences` doc).
pub const CREDIT_PREFERENCE_OPTIONS_FEATURE: &str = "CREDIT_PREFERENCE_OPTIONS";

/// The first segment of a `CREDIT_PREFERENCE_OPTIONS` key, from the documented
/// `credit/{creditId}/status` form.
pub const CREDIT_KEY_PREFIX: &str = "credit";

/// The one `CREDIT_PREFERENCE_OPTIONS` option that is NOT a sharing key: the
/// `UpdateBillingPreferences` doc singles out `credit/{creditId}/status` as
/// settable "by member accounts for credits they own, or by the management
/// account for any credit in the family".
pub const CREDIT_STATUS_OPTION: &str = "status";

/// The suffix the model's own enum uses for a read-only history feature:
/// `BILLING_FEATURE_VALUES` pairs `RI_SHARING` with `RI_SHARING_HISTORY` and
/// `CREDIT_SHARING` with `CREDIT_SHARING_HISTORY`, and the
/// `UpdateBillingPreferences` `feature` doc says those two "are read-only and
/// cannot be updated".
pub const HISTORY_FEATURE_SUFFIX: &str = "_HISTORY";

/// The length of an `AccountId`, whose modelled pattern is `^[0-9]{12}$`.
const ACCOUNT_ID_LEN: usize = 12;

/// The display name the emulator's own account carries inside its consolidated
/// billing family, which `BillingPreferenceSummary.accountName` reports beside
/// `accountId`. The account exists before any API call, exactly as the default
/// `PRIMARY` billing view does with [`PRIMARY_VIEW_NAME`].
pub const MANAGEMENT_ACCOUNT_NAME: &str = "Management Account";

/// The most billing-preference keys this emulator keeps resident.
///
/// `billing_preferences` grows with every new key `UpdateBillingPreferences`
/// sets, so it needs a ceiling for the same reason `billing_views` does. This is
/// an emulator resource bound, not a published AWS quota: past it a request that
/// would add a new key is refused with the modeled `ValidationException` rather
/// than letting a caller's request rate size the process.
pub const MAX_BILLING_PREFERENCES: usize = 1024;

/// The most keys one `UpdateBillingPreferences` request may carry.
///
/// `billingPreferencesPerKey` is modelled `@length(min: 1)` — a minimum with no
/// maximum — so the entry count is a caller-chosen factor the boundary validator
/// cannot bound. Another emulator resource bound, not a published AWS quota: a
/// request past it is refused with the modeled `ValidationException` before any
/// per-entry work, so no caller can choose how long a handler runs.
pub const MAX_KEYS_PER_UPDATE: usize = 100;

/// The most preference changes the history features report.
///
/// Also an emulator residency bound: beyond it the OLDEST change is dropped, so
/// the history stays the most recent window of changes instead of growing
/// without limit.
pub const MAX_PREFERENCE_HISTORY: usize = 1024;

/// One recorded change to a billing preference, as a history feature reports it.
#[derive(Debug, Clone)]
pub struct PreferenceChange {
    /// This change's place in the record, assigned when it was appended and
    /// never reused — see
    /// [`ComAmazonawsBillingAwsbillingState::next_preference_change_seq`]. A
    /// recorded change carries no other unique value (one key can change many
    /// times, even within one clock tick), so this is what names its position
    /// to a paginated read.
    pub seq: u64,
    /// The feature whose preference changed (e.g. `RI_SHARING`), NOT the history
    /// feature that reports the change.
    pub feature: String,
    pub key: String,
    /// The `PreferenceValue` the key was set to.
    pub value: String,
    pub at: SystemTime,
}

/// The cursor value that names a recorded change's position in a history
/// feature's sequence.
///
/// Zero-padded to the widest [`PreferenceChange::seq`] there can be, so the
/// cursors compare as text in the same order as the changes they name and one
/// read can hold the cursors of both kinds of feature as strings.
fn change_cursor(seq: u64) -> String {
    format!("{seq:020}")
}

/// One entry `GetBillingPreferences` reports, built from stored state.
#[derive(Debug, Clone)]
pub struct PreferenceEntry {
    /// What names this entry's position in the sequence
    /// [`ComAmazonawsBillingAwsbillingState::preference_entries`] walks, so a
    /// page can resume strictly after the entry the previous page ended on: the
    /// preference key for a feature whose entries are the stored keys (unique,
    /// and in the walk's own order), and [`change_cursor`] of the recorded
    /// change for a history feature. Ordered as text, ascending with the walk,
    /// and never reported on the wire.
    pub cursor: String,
    /// The feature the request asked for.
    pub feature: String,
    pub key: String,
    pub value: String,
    /// The account an account-list key names, with its display name — both set
    /// together or both absent, as `BillingPreferenceSummary` documents.
    pub account: Option<(String, String)>,
    /// The `(year, month)` billing period of a recorded change. Set only for the
    /// history features, per `BillingPreferenceSummary.billingPeriod`.
    pub billing_period: Option<(i32, i32)>,
}

/// One persisted billing view.
///
/// `sourceViewCount`, `derivedViewCount` and `sourceAccountId` are NOT fields:
/// the first is `source_views.len()`, the second is the number of views whose
/// `source_views` name this one, and the third is the account the view's own
/// sources belong to ([`BillingView::source_account_id`]) — all three derived on
/// read so none of them can go stale.
#[derive(Debug, Clone)]
pub struct BillingView {
    /// The ARN this view is keyed by, minted by [`ComAmazonawsBillingAwsbillingState::mint_view_arn`].
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// A `BillingViewType` value, e.g. [`BILLING_VIEW_TYPE_CUSTOM`].
    pub billing_view_type: String,
    /// The 12-digit account that owns the view.
    pub owner_account_id: String,
    /// The billing-view ARNs this view draws its data from.
    pub source_views: Vec<String>,
    pub data_filter_expression: Option<Expression>,
    pub created_at: SystemTime,
    pub updated_at: Option<SystemTime>,
    pub view_definition_last_updated_at: Option<SystemTime>,
    /// A `BillingViewStatus` value, e.g. [`BILLING_VIEW_STATUS_HEALTHY`].
    pub health_status_code: String,
    /// The `BillingViewStatusReason` values explaining
    /// [`BillingView::health_status_code`], e.g. `SOURCE_VIEW_UNHEALTHY`
    /// (`BILLING_VIEW_STATUS_REASON_VALUES` in `ops::types`).
    ///
    /// A list, not an `Option`: the health status a read reports always carries
    /// both members, and a status that needs no explaining — a `HEALTHY` view —
    /// carries the EMPTY list rather than an absent one.
    pub health_status_reasons: Vec<String>,
    /// The view's resource tags, by key.
    pub tags: BTreeMap<String, String>,
    /// The resource-based policy document attached to this view, as
    /// `GetResourcePolicy` reports it.
    ///
    /// `None` for every view this emulator creates: only a CUSTOM billing view
    /// can carry one, and this service's model declares no operation that
    /// attaches a policy (there is no `PutResourcePolicy` — on AWS the document
    /// appears when the view is shared), so nothing in the API writes it.
    /// `GetResourcePolicy` reports the stored document when a view carries one
    /// and answers `ResourceNotFoundException` when it does not.
    pub resource_policy: Option<String>,
}

impl BillingView {
    /// Whether this view is one AWS manages on the account's behalf rather than
    /// one a customer created.
    ///
    /// `CreateBillingView` is the only way a customer makes a view and it always
    /// produces [`BILLING_VIEW_TYPE_CUSTOM`]; every other `BillingViewType`
    /// value (`PRIMARY`, `BILLING_GROUP`, `BILLING_TRANSFER`,
    /// `BILLING_TRANSFER_SHOWBACK`) names a view the service itself maintains,
    /// whose tags and definition the customer does not own.
    pub fn is_managed(&self) -> bool {
        self.billing_view_type != BILLING_VIEW_TYPE_CUSTOM
    }

    /// Whether this view is active for as long as the account is, so no time
    /// window a caller asks about can exclude it.
    ///
    /// Only the account's default [`BILLING_VIEW_TYPE_PRIMARY`] view is: the
    /// `ListBillingViews` `activeTimeRange` doc says it "is always listed",
    /// because it is not a resource that came into existence at an instant — it
    /// represents the account's own billing data. Every other type names a
    /// resource with a lifetime (a Billing Conductor billing group, a view a
    /// caller created), and is active only from the instant that resource
    /// appeared.
    pub fn is_always_active(&self) -> bool {
        self.billing_view_type == BILLING_VIEW_TYPE_PRIMARY
    }

    /// Whether this view's `BillingViewStatus` is
    /// [`BILLING_VIEW_STATUS_HEALTHY`], the one status in which an operation may
    /// act on it: every other value (`CREATING`, `UNHEALTHY`, `UPDATING`) is a
    /// state the modeled `BillingViewHealthStatusException` names — see
    /// [`billing_view_not_healthy`].
    pub fn is_healthy(&self) -> bool {
        self.health_status_code == BILLING_VIEW_STATUS_HEALTHY
    }

    /// The accounts owning the billing views this one draws its data from, in
    /// the order [`BillingView::source_views`] holds them: the account id each
    /// source ARN embeds, skipping an ARN that embeds none.
    pub fn source_accounts(&self) -> impl Iterator<Item = &str> {
        self.source_views
            .iter()
            .filter_map(|arn| source_view_account(arn))
    }

    /// `sourceAccountId` as a read reports it — "the Amazon Web Services account
    /// ID that owns the source billing view, if this is a derived billing view".
    ///
    /// A view is derived when it draws from source views, so this is the account
    /// owning the first billing view it draws from, and `None` for a view with
    /// no source at all. Computed from the stored sources on every read, exactly
    /// like `sourceViewCount` and `derivedViewCount`, so it can never disagree
    /// with the sources a caller reads back.
    ///
    /// The member is singular, so the documented sentence pins the answer only
    /// for a view whose sources all belong to one account; taking the FIRST
    /// source's account is this emulator's tie-break for a view drawing on
    /// several accounts' views, and the filter still selects such a view for
    /// EVERY account it draws on (see [`BillingView::draws_from_account`]).
    pub fn source_account_id(&self) -> Option<&str> {
        self.source_accounts().next()
    }

    /// Whether this view uses a billing view of `account_id` as a source — what
    /// the `ListBillingViews` `sourceAccountId` filter selects on ("only billing
    /// views that use the specified account as a source"). A view drawing from
    /// several accounts' views uses each of them as a source.
    pub fn draws_from_account(&self, account_id: &str) -> bool {
        self.source_accounts().any(|account| account == account_id)
    }
}

/// A monetary amount, as the modeled `Amount` shape carries it: an ISO-4217
/// `currencyCode` (`^[A-Z]{3}$`) and a decimal `currencyAmount`.
///
/// Held as the wire form rather than a float: the modeled `CurrencyAmount` is a
/// string, and money must not be rounded by a binary float on the way through
/// the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreditAmount {
    pub currency_code: String,
    pub currency_amount: String,
}

/// One persisted credit-allocation entry: how much of one credit was applied to
/// one Amazon Web Services service on one account's bill for one billing month.
///
/// The members mirror the modeled `CreditAllocationHistoryEntry`, so
/// `GetCreditAllocationHistory` reports what was persisted rather than
/// recomputing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreditAllocation {
    /// The `CreditId` of the credit that was applied (1..32 digits), as
    /// [`Credit::credit_id`] holds it.
    pub credit_id: String,
    /// The amount of the credit this entry applied.
    pub credit_amount: CreditAmount,
    pub description: Option<String>,
    /// The 12-digit account whose bill the credit was applied to.
    pub account_id: String,
    /// The Amazon Web Services service the credit was applied to.
    pub applied_service_name: String,
    /// The billing month of the application, in the modeled `BillingMonth`
    /// `YYYY-MM` form — the first segment of [`CreditAllocation::key`].
    pub billing_month: String,
    /// `true` when the entry was applied to a bill that is not yet finalized.
    pub is_estimated_bill: bool,
}

impl CreditAllocation {
    /// The most credit-allocation entries this emulator keeps resident.
    ///
    /// `credit_allocations` grows with every credit applied to a bill, so it
    /// needs a ceiling for the same reason `billing_views` does. An emulator
    /// resource bound, not a published AWS quota:
    /// [`ComAmazonawsBillingAwsbillingState::record_credit_allocation`] refuses
    /// a write past it rather than growing without limit.
    pub const MAX_RESIDENT: usize = 4096;

    /// The key this allocation is stored under: billing month, then credit id,
    /// then account, then applied service — the members that together identify
    /// one application of one credit.
    ///
    /// The credit id is left-padded to the modeled `CreditId` maximum of 32
    /// digits so ids order numerically, and the parts are joined with the ASCII
    /// unit separator (`0x1F`), which sorts below every character the modeled
    /// members can carry. Ascending key order is therefore chronological, and a
    /// reverse walk is the `billingMonth`-descending order
    /// `GetCreditAllocationHistory` documents.
    pub fn key(&self) -> String {
        format!(
            "{}\u{1f}{:0>32}\u{1f}{}\u{1f}{}",
            self.billing_month, self.credit_id, self.account_id, self.applied_service_name,
        )
    }

    /// The lowest [`CreditAllocation::key`] of the billing month `month`, and
    /// the lowest key ABOVE every key of that month — the inclusive lower and
    /// exclusive upper bounds of one month's entries in `credit_allocations`.
    ///
    /// A month is a fixed-width `YYYY-MM` prefix followed by the key separator
    /// (`0x1F`), so the month string itself sorts below every key of that month
    /// and the same string followed by the next character (a space) sorts above
    /// them all and below every key of a later month. A reader can therefore
    /// seek straight to a month range instead of scanning the whole store.
    pub fn month_key_bounds(month: &str) -> (String, String) {
        (month.to_string(), format!("{month}\u{20}"))
    }
}

/// One persisted account credit — the record `GetCredits` reports.
///
/// Mirrors the modeled `CreditData` shape, with instants kept as
/// [`SystemTime`] (converted on read by [`timestamp`]) so that stored and
/// requested periods compare on one scale. Enum-typed members are held as the
/// model's own string values (`ApplicationType`, `CreditSharingType`,
/// `CreditStatus`).
#[derive(Debug, Clone)]
pub struct Credit {
    /// The modeled `CreditId`: 1..32 characters matching `^[0-9]+$`. This is
    /// the key the credit is stored under.
    pub credit_id: String,
    /// The 12-digit account that owns the credit.
    pub account_id: String,
    pub credit_type: String,
    pub initial_amount: CreditAmount,
    pub remaining_amount: CreditAmount,
    pub estimated_amount: Option<CreditAmount>,
    /// The products the credit applies to; empty when it applies to any.
    pub applicable_product_names: Vec<String>,
    pub description: String,
    /// When the credit becomes applicable.
    pub start_date: SystemTime,
    /// When the credit expires; `None` for a credit that does not expire.
    pub end_date: Option<SystemTime>,
    /// When the credit was fully consumed, if it has been.
    pub exhaust_date: Option<SystemTime>,
    /// An `ApplicationType` value, e.g. `AFTER_DISCOUNTS`.
    pub application_type: Option<String>,
    /// The accounts the credit may be shared with.
    pub shareable_accounts: Vec<String>,
    pub account_has_credit_sharing_enabled: Option<bool>,
    pub credit_console_visibility: Option<String>,
    /// A `CreditSharingType` value, e.g. `DEFAULT`.
    pub credit_sharing_type: Option<String>,
    pub cost_category_arn: Option<String>,
    pub rule_name: Option<String>,
    /// A `CreditStatus` value: `ENABLED` or `DISABLED`.
    pub credit_status: Option<String>,
    /// The purchase types the credit applies to.
    pub purchase_type_applications: Vec<String>,
}

impl Credit {
    /// Whether this credit's own life overlaps the closed period
    /// `[start_secs, end_secs]`, both on [`epoch_secs`]' scale.
    ///
    /// A credit with no `end_date` never expires, so it overlaps every period
    /// that ends on or after it started.
    pub fn overlaps_period(&self, start_secs: f64, end_secs: f64) -> bool {
        epoch_secs(self.start_date) <= end_secs
            && self
                .end_date
                .is_none_or(|end| epoch_secs(end) >= start_secs)
    }
}

/// Per-service state for com-amazonaws-billing-awsbilling. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsBillingAwsbillingState {
    /// Every persisted billing view, keyed and ordered by ARN. Bounded by
    /// [`MAX_BILLING_VIEWS`].
    pub billing_views: BTreeMap<String, BillingView>,
    /// Every persisted credit allocation, keyed and ordered by
    /// [`CreditAllocation::key`] so `GetCreditAllocationHistory`'s page
    /// boundaries are stable. Bounded by [`CreditAllocation::MAX_RESIDENT`].
    ///
    /// Empty on a fresh emulator: an allocation is written when a credit is
    /// applied to a bill, and this emulator runs no billing pipeline, so only a
    /// future operation that applies credits adds entries here.
    pub credit_allocations: BTreeMap<String, CreditAllocation>,
    /// `clientToken` -> the ARN its first `CreateBillingView` minted, so a
    /// replayed create returns that ARN instead of making a second view.
    /// Bounded by [`MAX_IDEMPOTENCY_TOKENS`]: an entry is reclaimed by
    /// [`Self::forget_view`] when its view is deleted.
    pub idempotency_tokens: HashMap<String, String>,
    /// Source of the next `billingview/custom-<n>` resource id. Used ONLY to
    /// build an ARN that is then stored in `billing_views`.
    pub next_view_id: u64,
    /// Every billing preference the account has set, keyed by
    /// `(BillingFeature, PreferenceKey)` and so ordered by feature then key.
    /// Bounded by [`MAX_BILLING_PREFERENCES`].
    pub billing_preferences: BTreeMap<(String, String), String>,
    /// The record of preference changes, oldest first — what the history
    /// features report. Bounded by [`MAX_PREFERENCE_HISTORY`], dropping the
    /// oldest change first.
    pub preference_history: VecDeque<PreferenceChange>,
    /// Source of the next [`PreferenceChange::seq`]. Only ever advances, so a
    /// recorded change's position stays its own even after the oldest changes
    /// are dropped from the front of `preference_history`.
    pub next_preference_change_seq: u64,
    /// The consolidated billing family: account id -> that account's display
    /// name, which `accountName` reports beside `accountId`.
    pub consolidated_billing_family: BTreeMap<String, String>,
    /// The management (payer) account of that consolidated billing family —
    /// per the `UpdateBillingPreferences` doc, the only identity that may set a
    /// sharing preference.
    ///
    /// This is the emulated account itself: a standalone account is the payer
    /// account of its own family, which is NOT the same as being an AWS
    /// Organizations management account (see
    /// [`Self::organization_management_account_id`], `None` by default).
    pub billing_family_management_account_id: String,
    /// The management account of the AWS Organizations organization this
    /// emulated account belongs to, when it belongs to one.
    ///
    /// Creating a `CUSTOM` billing view is an organization-management-account
    /// action (see the `UpdateBillingPreferences` doc for the same split:
    /// sharing keys "may only be set by the management account of a
    /// consolidated billing family"), so `CreateBillingView` consults this
    /// before it mutates anything.
    ///
    /// `None` by default: an organization is established through AWS
    /// Organizations, which this emulator models in its own service and whose
    /// state holds no organization until one is created — so the emulated
    /// account is a standalone account and is nobody's organization management
    /// account. That is NOT the same as being the payer account of its own
    /// consolidated billing family, which a standalone account is (see
    /// [`Self::credits_for_period`]). Set this to the caller's account id to
    /// model the management account of an organization.
    pub organization_management_account_id: Option<String>,
    /// Every credit held by an account of this emulator's consolidated billing
    /// family, keyed and ordered by `creditId`, so `GetCredits` answers in a
    /// deterministic order. Bounded by [`MAX_CREDITS`].
    pub credits: BTreeMap<String, Credit>,
    /// Every persisted account-month of Support-eligible spend, keyed and
    /// ordered by [`SupportEligibleSpend::key`] (billing month, then account)
    /// so `ListBusinessSupportAccountCharges` seeks straight to one month and
    /// its page boundaries are stable. Bounded by
    /// [`SupportEligibleSpend::MAX_RESIDENT`].
    ///
    /// Empty on a fresh emulator: spend accrues from a billing run, which this
    /// emulator does not perform. The Support CHARGE is not held here — it is
    /// computed from this spend by the subscribed plan's price schedule.
    pub support_eligible_spend: BTreeMap<String, SupportEligibleSpend>,
    /// The Enterprise Support contract the consolidated billing family's
    /// Support profile is billed under: the pricing plans
    /// `GetEnterpriseSupportChargeSummary` prices the profile's
    /// Support-eligible spend by, and the method that distributes the charge
    /// across the accounts it covers.
    pub enterprise_support_contract: EnterpriseSupportContract,
    /// When each account of the consolidated billing family was linked to the
    /// payer account and subscribed to Enterprise Support, keyed and ordered by
    /// account id — the intervals
    /// `ListEnterpriseSupportLinkedAccountCharges` prorates a month's spend by.
    /// Bounded by [`EnterpriseSupportLinkedAccount::MAX_RESIDENT`].
    ///
    /// Empty on a fresh emulator: an account is linked and subscribed outside
    /// this service, so an account with no record here is reported with the
    /// whole billing period billable (see the `linked_accounts` module).
    pub enterprise_support_linked_accounts: BTreeMap<String, EnterpriseSupportLinkedAccount>,
    /// The `BillingFeature` values whose preferences this account's caller may
    /// reach — read through `GetBillingPreferences` and write through
    /// `UpdateBillingPreferences`.
    ///
    /// Seeded with every feature except [`ACCESS_ACTIVATION_FEATURES`], the
    /// ones behind a Billing-console access activation this account does not
    /// have. Both preference operations consult it BEFORE they touch
    /// `billing_preferences`, so a gated feature is refused whether or not a
    /// preference is stored under it, and a gated write stores nothing.
    pub feature_access: BTreeSet<String>,
}

impl Default for ComAmazonawsBillingAwsbillingState {
    fn default() -> Self {
        // The account's default PRIMARY view exists before any API call: see
        // BILLING_VIEW_TYPE_PRIMARY for the `ListBillingViews` doc sentence.
        let now = SystemTime::now();
        let owner_account_id = ::engine::emulator_account_id().to_string();
        let primary = BillingView {
            arn: billing_view_arn(PRIMARY_VIEW_ID),
            name: PRIMARY_VIEW_NAME.to_string(),
            description: Some(primary_view_description(&owner_account_id)),
            billing_view_type: BILLING_VIEW_TYPE_PRIMARY.to_string(),
            owner_account_id,
            source_views: Vec::new(),
            data_filter_expression: None,
            created_at: now,
            updated_at: None,
            view_definition_last_updated_at: None,
            health_status_code: BILLING_VIEW_STATUS_HEALTHY.to_string(),
            health_status_reasons: Vec::new(),
            tags: BTreeMap::new(),
            resource_policy: None,
        };
        let mut billing_views = BTreeMap::new();
        billing_views.insert(primary.arn.clone(), primary);
        // The emulator serves one account, and it is the management account of
        // its own consolidated billing family: `caller_account_id` signs every
        // request with it.
        let mut consolidated_billing_family = BTreeMap::new();
        consolidated_billing_family.insert(
            caller_account_id().to_string(),
            MANAGEMENT_ACCOUNT_NAME.to_string(),
        );
        Self {
            billing_views,
            // No credit has been applied to a bill yet, so the account has no
            // allocation history: AWS documents no credit every account owns
            // by default, the way it documents the PRIMARY billing view.
            credit_allocations: BTreeMap::new(),
            idempotency_tokens: HashMap::new(),
            next_view_id: 1,
            billing_preferences: BTreeMap::new(),
            preference_history: VecDeque::new(),
            next_preference_change_seq: 1,
            consolidated_billing_family,
            billing_family_management_account_id: caller_account_id().to_string(),
            organization_management_account_id: None,
            // A fresh account holds no credits: the modeled `GetCredits`
            // response "returns an empty list when no credits exist".
            credits: BTreeMap::new(),
            // Nothing has been billed yet, so no account has Support-eligible
            // spend: AWS documents no charge every account carries by
            // default, the way it documents the PRIMARY billing view.
            support_eligible_spend: BTreeMap::new(),
            // The published Enterprise Support price schedule, in effect for
            // every billing month until a negotiated plan is stored beside it:
            // a profile is billed under the standard plan unless its contract
            // names another.
            enterprise_support_contract: EnterpriseSupportContract::default(),
            // No account's linkage or Support subscription has been recorded:
            // both happen outside this service, so a month's bill is the only
            // evidence of a subscription until one is.
            enterprise_support_linked_accounts: BTreeMap::new(),
            // Every feature except the ones behind the Billing-console access
            // activation this account does not have.
            feature_access: BILLING_FEATURE_VALUES
                .iter()
                .filter(|feature| !ACCESS_ACTIVATION_FEATURES.contains(*feature))
                .map(|feature| (*feature).to_string())
                .collect(),
        }
    }
}

impl ComAmazonawsBillingAwsbillingState {
    /// Mint an unused billing-view ARN and advance the id source.
    ///
    /// The returned ARN is not stored by this call — the caller inserts the
    /// record it belongs to.
    pub fn mint_view_arn(&mut self) -> String {
        loop {
            let arn = billing_view_arn(&format!("custom-{}", self.next_view_id));
            self.next_view_id += 1;
            if !self.billing_views.contains_key(&arn) {
                return arn;
            }
        }
    }

    /// The ARNs of the views that use `arn` as one of their source views —
    /// the resources a delete would break, and the count `derivedViewCount`
    /// reports.
    pub fn derived_view_arns(&self, arn: &str) -> Vec<String> {
        self.billing_views
            .values()
            .filter(|view| view.source_views.iter().any(|source| source == arn))
            .map(|view| view.arn.clone())
            .collect()
    }

    /// Drop the view at `arn` and every reference the store holds to it: its own
    /// record, every idempotency token that named it, and its place in every
    /// other view's source-view set.
    ///
    /// Those three are all a billing view is referenced by, so afterwards no
    /// stored record names a view this store no longer holds: a create/delete
    /// loop reclaims every token it allocated instead of leaving entries behind
    /// that no API can observe or free, and no read can report an association
    /// that points at nothing — the state `AssociateSourceViews` refuses to
    /// write.
    ///
    /// A view that drew from `arn` loses that one source and keeps the rest in
    /// the order it held them. Its source set is part of its definition, so the
    /// two instants a definition change moves — `updatedAt` and
    /// `viewDefinitionLastUpdatedAt`, the pair `AssociateSourceViews` and
    /// `DisassociateSourceViews` move when they widen or narrow the same set —
    /// move to `at`, and a view whose set did not contain `arn` is left
    /// untouched.
    pub fn forget_view(&mut self, arn: &str, at: SystemTime) -> Option<BillingView> {
        let removed = self.billing_views.remove(arn);
        self.idempotency_tokens.retain(|_, stored| stored != arn);
        for view in self.billing_views.values_mut() {
            if !view.source_views.iter().any(|source| source == arn) {
                continue;
            }
            view.source_views.retain(|source| source != arn);
            view.updated_at = Some(at);
            view.view_definition_last_updated_at = Some(at);
        }
        removed
    }

    /// Persist one credit allocation under its [`CreditAllocation::key`],
    /// replacing an entry with the same key (same credit, account, service and
    /// billing month) rather than duplicating it.
    ///
    /// Answers `false` without writing when the store is already at
    /// [`CreditAllocation::MAX_RESIDENT`], so the caller refuses rather than
    /// growing the store without a bound. The only writer today is this
    /// service's test suite: a credit is APPLIED to a bill by a billing run,
    /// which this emulator does not perform, so a future operation that applies
    /// credits is what will call this in production.
    pub fn record_credit_allocation(&mut self, allocation: CreditAllocation) -> bool {
        let key = allocation.key();
        if !self.credit_allocations.contains_key(&key)
            && self.credit_allocations.len() >= CreditAllocation::MAX_RESIDENT
        {
            return false;
        }
        self.credit_allocations.insert(key, allocation);
        true
    }

    /// Whether `needle` is `start` itself or is reachable from `start` by
    /// following source-view links, i.e. whether making `start` a source of
    /// `needle` would put `needle` in its own source chain.
    ///
    /// The walk keeps a visited set, so a chain this store already holds cannot
    /// make it loop, and each view is expanded once however many views name it.
    pub fn draws_from(&self, start: &str, needle: &str) -> bool {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut pending: Vec<&str> = vec![start];
        while let Some(arn) = pending.pop() {
            if arn == needle {
                return true;
            }
            if !seen.insert(arn) {
                continue;
            }
            if let Some(view) = self.billing_views.get(arn) {
                pending.extend(view.source_views.iter().map(String::as_str));
            }
        }
        false
    }

    /// Whether another view fits under [`MAX_BILLING_VIEWS`], counting the
    /// idempotency entry it would write against [`MAX_IDEMPOTENCY_TOKENS`].
    pub fn has_room_for_another_view(&self, writes_token: bool) -> bool {
        self.billing_views.len() < MAX_BILLING_VIEWS
            && (!writes_token || self.idempotency_tokens.len() < MAX_IDEMPOTENCY_TOKENS)
    }

    /// Whether `account_id` is the management (payer) account of the
    /// consolidated billing family — the only identity that may set a sharing
    /// preference. Distinct from
    /// [`Self::is_organization_management_account`]: a standalone account is its
    /// own family's payer account but nobody's organization management account.
    pub fn is_billing_family_management_account(&self, account_id: &str) -> bool {
        self.billing_family_management_account_id == account_id
    }

    /// The display name of a member of the consolidated billing family, or
    /// `None` for an account outside it.
    pub fn family_account_name(&self, account_id: &str) -> Option<&str> {
        self.consolidated_billing_family
            .get(account_id)
            .map(String::as_str)
    }

    /// Whether a record about the bill of `billed_account` belongs to the
    /// billing history reported FOR `account_id`.
    ///
    /// An account's own bills always; and, when `account_id` is the management
    /// (payer) account of the consolidated billing family, the bill of every
    /// MEMBER of that family too — the family traversal that lets a credit
    /// applied to a member's bill appear in the payer's history. An account
    /// that pays for nobody (a member account, or an account outside the
    /// family) has only its own bills: a family's history is the payer's to
    /// read, not a member's.
    pub fn history_covers_billed_account(&self, account_id: &str, billed_account: &str) -> bool {
        billed_account == account_id
            || (self.is_billing_family_management_account(account_id)
                && self.family_account_name(billed_account).is_some())
    }

    /// How many of `keys` are not stored under `feature` yet — the number of
    /// entries an update would ADD, which is what [`MAX_BILLING_PREFERENCES`]
    /// bounds.
    ///
    /// Walks the stored keys once and strikes each one off a set of the
    /// requested keys, so the cost is one ordered pass over a store that
    /// [`MAX_BILLING_PREFERENCES`] bounds plus a set lookup per stored key —
    /// never a scan per requested key, and no allocation per probe.
    pub fn new_preference_keys<'a>(
        &self,
        feature: &str,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> usize {
        let mut absent: BTreeSet<&str> = keys.into_iter().collect();
        for (stored_feature, stored_key) in self.billing_preferences.keys() {
            if stored_feature == feature {
                absent.remove(stored_key.as_str());
            }
        }
        absent.len()
    }

    /// Set one preference, leaving every key the request did not name unchanged,
    /// and record the change when the feature has a history feature.
    ///
    /// A write that does not move the value records nothing: the history is a
    /// record of CHANGES.
    pub fn set_preference(&mut self, feature: &str, key: &str, value: &str, at: SystemTime) {
        let slot = (feature.to_string(), key.to_string());
        let changed = self.billing_preferences.get(&slot).map(String::as_str) != Some(value);
        self.billing_preferences.insert(slot, value.to_string());
        if !changed || history_feature_for(feature).is_none() {
            return;
        }
        let seq = self.next_preference_change_seq;
        self.next_preference_change_seq += 1;
        self.preference_history.push_back(PreferenceChange {
            seq,
            feature: feature.to_string(),
            key: key.to_string(),
            value: value.to_string(),
            at,
        });
        while self.preference_history.len() > MAX_PREFERENCE_HISTORY {
            self.preference_history.pop_front();
        }
    }

    /// Every entry the feature `feature` reports, in the order
    /// `GetBillingPreferences` pages through them.
    ///
    /// For a history feature that is the recorded changes to the feature behind
    /// it, oldest first; for every other feature it is the stored keys in key
    /// order. A feature nothing has set reports nothing: the model documents no
    /// key catalog per feature, so the emulator invents no entry for one.
    ///
    /// Every entry carries the [`PreferenceEntry::cursor`] that names its place
    /// in this sequence, and those cursors ASCEND with it — the walk is a
    /// `BTreeMap` in key order or the record of changes in the order they were
    /// recorded — so a paginated read resumes after one by comparing cursors
    /// instead of counting entries it already delivered.
    pub fn preference_entries(&self, feature: &str) -> Vec<PreferenceEntry> {
        if let Some(behind) = feature_behind_history(feature) {
            return self
                .preference_history
                .iter()
                .filter(|change| change.feature == behind)
                .map(|change| PreferenceEntry {
                    cursor: change_cursor(change.seq),
                    feature: feature.to_string(),
                    key: change.key.clone(),
                    value: change.value.clone(),
                    account: self.account_of_key(&change.key),
                    billing_period: billing_period(change.at),
                })
                .collect();
        }
        self.billing_preferences
            .iter()
            .filter(|((stored, _), _)| stored == feature)
            .map(|((_, key), value)| PreferenceEntry {
                cursor: key.clone(),
                feature: feature.to_string(),
                key: key.clone(),
                value: value.clone(),
                account: self.account_of_key(key),
                billing_period: None,
            })
            .collect()
    }

    /// The account id and display name an account-list key names, when the
    /// account is one this family holds — the only case in which
    /// `accountId`/`accountName` can both be reported, as the model requires.
    fn account_of_key(&self, key: &str) -> Option<(String, String)> {
        let account_id = account_list_key_account(key)?;
        let name = self.family_account_name(account_id)?;
        Some((account_id.to_string(), name.to_string()))
    }

    /// Whether this account's caller may reach the preferences of `feature`.
    ///
    /// The one access decision both preference operations make, so a read and a
    /// write of the same feature can never disagree about it.
    pub fn has_feature_access(&self, feature: &str) -> bool {
        self.feature_access.contains(feature)
    }

    /// Activate `feature` for this account, so its preferences become
    /// reachable. Idempotent.
    pub fn grant_feature_access(&mut self, feature: &str) {
        self.feature_access.insert(feature.to_string());
    }

    /// Withdraw `feature`'s activation, so its preferences are refused again.
    /// Idempotent.
    pub fn revoke_feature_access(&mut self, feature: &str) {
        self.feature_access.remove(feature);
    }

    /// Whether `account_id` is the management account of the organization this
    /// account belongs to — see [`Self::organization_management_account_id`]
    /// for why a standalone account is not.
    pub fn is_organization_management_account(&self, account_id: &str) -> bool {
        self.organization_management_account_id.as_deref() == Some(account_id)
    }

    /// The stored view `arn` names when it is one this account may edit or
    /// delete: an existing `CUSTOM` view.
    ///
    /// `None` covers both inputs AWS collapses into one refusal — an ARN no
    /// view is stored under, and one naming a view that is not `CUSTOM` (the
    /// account's `PRIMARY` view) — so eligibility is decided without revealing
    /// which of the two it was.
    pub fn custom_view(&self, arn: &str) -> Option<&BillingView> {
        self.billing_views
            .get(arn)
            .filter(|view| view.billing_view_type == BILLING_VIEW_TYPE_CUSTOM)
    }

    /// [`Self::custom_view`], for a caller that goes on to change the record.
    pub fn custom_view_mut(&mut self, arn: &str) -> Option<&mut BillingView> {
        self.billing_views
            .get_mut(arn)
            .filter(|view| view.billing_view_type == BILLING_VIEW_TYPE_CUSTOM)
    }

    /// Store `credit` under its `creditId`, replacing any credit already held
    /// under that id. Answers `false` — storing nothing — when the store is
    /// already at [`MAX_CREDITS`], so a caller can refuse with a quota error
    /// rather than growing the process without bound.
    pub fn insert_credit(&mut self, credit: Credit) -> bool {
        if !self.credits.contains_key(&credit.credit_id) && self.credits.len() >= MAX_CREDITS {
            return false;
        }
        self.credits.insert(credit.credit_id.clone(), credit);
        true
    }

    /// The credits `GetCredits` reports, in `creditId` order.
    ///
    /// `account_id` selects the owning account. `whole_family` widens the
    /// selection to every account of the consolidated billing family — every
    /// account this store knows, since the emulator's own account
    /// (`engine::emulator_account_id`) is the payer account of its own
    /// consolidated billing family. Both bounds of the requested period are
    /// inclusive, on [`epoch_secs`]' scale, and a credit is reported when its
    /// own life overlaps it.
    ///
    /// A pure read: it answers from the store and changes nothing in it.
    pub fn credits_for_period(
        &self,
        account_id: &str,
        whole_family: bool,
        start_secs: f64,
        end_secs: f64,
    ) -> Vec<Credit> {
        self.credits
            .values()
            .filter(|credit| whole_family || credit.account_id == account_id)
            .filter(|credit| credit.overlaps_period(start_secs, end_secs))
            .cloned()
            .collect()
    }
}

/// The account every caller of this emulator authenticates as: every request
/// is signed with the emulator's own credentials, so the caller's account is
/// [`engine::emulator_account_id`] — and therefore the account whose
/// preferences a request reads or writes.
pub fn caller_account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// Whether `feature` is one of the model's read-only history features.
pub fn is_history_feature(feature: &str) -> bool {
    feature_behind_history(feature).is_some()
}

/// The feature whose changes `feature` records, when `feature` is a history
/// feature the model declares.
pub fn feature_behind_history(feature: &str) -> Option<&str> {
    let behind = feature.strip_suffix(HISTORY_FEATURE_SUFFIX)?;
    BILLING_FEATURE_VALUES.contains(&behind).then_some(behind)
}

/// The history feature that records changes to `feature`, when the model
/// declares one (`RI_SHARING` -> `RI_SHARING_HISTORY`).
pub fn history_feature_for(feature: &str) -> Option<String> {
    let history = format!("{feature}{HISTORY_FEATURE_SUFFIX}");
    BILLING_FEATURE_VALUES
        .contains(&history.as_str())
        .then_some(history)
}

/// The `creditId` and option a `CREDIT_PREFERENCE_OPTIONS` key names, for a key
/// in the documented `credit/{creditId}/{option}` form; `None` for any other
/// shape of key.
pub fn credit_key_parts(key: &str) -> Option<(&str, &str)> {
    let mut segments = key.split('/');
    let prefix = segments.next()?;
    let credit_id = segments.next()?;
    let option = segments.next()?;
    let extra = segments.next();
    if prefix != CREDIT_KEY_PREFIX || credit_id.is_empty() || option.is_empty() || extra.is_some() {
        return None;
    }
    Some((credit_id, option))
}

/// Whether `key` under `feature` is a sharing key: every key of a sharing
/// feature, and every `CREDIT_PREFERENCE_OPTIONS` option other than
/// [`CREDIT_STATUS_OPTION`].
pub fn is_sharing_key(feature: &str, key: &str) -> bool {
    if SHARING_FEATURES.contains(&feature) {
        return true;
    }
    feature == CREDIT_PREFERENCE_OPTIONS_FEATURE
        && credit_key_parts(key).is_some_and(|(_, option)| option != CREDIT_STATUS_OPTION)
}

/// Whether `value` is an account id: exactly the 12 digits the modeled
/// `AccountId` shape (`^[0-9]{12}$`) admits.
pub fn is_account_id(value: &str) -> bool {
    value.len() == ACCOUNT_ID_LEN && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// The account an account-list key names: the key's last `/`-separated segment
/// when it is a 12-digit account id (`AccountId`'s modelled `^[0-9]{12}$`).
pub fn account_list_key_account(key: &str) -> Option<&str> {
    let last = key.split('/').next_back()?;
    is_account_id(last).then_some(last)
}

/// The billing period an instant falls in, as the modeled `BillingPeriod`
/// `(year, month)`. `None` only when the clock cannot be rendered.
pub fn billing_period(at: SystemTime) -> Option<(i32, i32)> {
    // `Format::DateTime` renders RFC 3339 in UTC, `YYYY-MM-DDThh:mm:ss[.fff]Z`,
    // so the year is the first four characters and the month the sixth and
    // seventh.
    let rendered = ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()?;
    let year = rendered.get(0..4)?.parse().ok()?;
    let month = rendered.get(5..7)?.parse().ok()?;
    Some((year, month))
}

/// The account id an ARN embeds (its fifth colon-separated segment), when it
/// has one.
pub fn arn_account_id(arn: &str) -> Option<&str> {
    arn.split(':').nth(4)
}

/// The account that owns the billing view `arn` names: the account id the ARN
/// embeds, when that segment is one (an ARN of a global, account-less resource
/// carries an empty segment there and so names no owner).
pub fn source_view_account(arn: &str) -> Option<&str> {
    arn_account_id(arn).filter(|account| is_account_id(account))
}

/// Whether `arn` is scoped to the caller's own account.
///
/// Every operation that identifies a billing view by ARN checks this BEFORE it
/// looks the ARN up, so the answer to a foreign-account ARN never depends on
/// whether a view exists there.
pub fn is_caller_account_arn(arn: &str) -> bool {
    arn_account_id(arn) == Some(caller_account_id())
}

/// The `AccessDeniedException` payload for an ARN outside the caller's
/// account: the account boundary is enforced before any existence lookup, so
/// the refusal is built from the caller's identity and the requested ARN and
/// says nothing about the resource.
pub fn cross_account_access_denied<Eng: Send + Sync + 'static>(
    operation: &str,
    arn: &str,
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> AccessDeniedException {
    AccessDeniedException {
        message: format!(
            "User: {} is not authorized to perform: {BILLING_SERVICE_CODE}:{operation} on \
             resource: {arn} because no resource-based policy allows the \
             {BILLING_SERVICE_CODE}:{operation} action",
            caller_identity_arn(engine, ctx)
        ),
    }
}

/// The ARN of the identity THIS request authenticates as, for the
/// `AccessDeniedException` message that names the denied principal.
///
/// Taken from the same source `sts:GetCallerIdentity` answers from — the STS
/// session the request's access key id names, or the ambient identity when it
/// names none — so the principal a refusal blames is the one a caller reads
/// back from STS for the credentials it signed with, rather than a second
/// identity minted here.
///
/// Falls back to the ambient identity, STS's own answer for a key that names no
/// session, when the session cannot be resolved (an expired credential, or an
/// engine with no STS store).
pub fn caller_identity_arn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> String {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    sts_state::service_state(engine)
        .and_then(|sts| {
            sts_state::resolve_caller(
                &mut sts.lock(),
                &access_key_id,
                SystemTime::now(),
                ctx.credential_store(),
            )
            .ok()
        })
        .unwrap_or_else(sts_state::ambient_identity)
        .arn
}

/// The suffix the billing-preference surface's BACKEND operation token carries
/// on top of the public operation name.
///
/// The bracketed `[Operation = …]` segment of this refusal reports the token of
/// the backend operation that made the decision, not the public API name the
/// caller invoked: the preference surface's backend operations are the public
/// names with this suffix appended (`GetBillingPreferences` is served by
/// `GetBillingPreferencesPaperwork`). It is a property of the surface, not of
/// any one run.
const PREFERENCES_BACKEND_SUFFIX: &str = "Paperwork";

/// The backend operation token the per-feature billing-access refusal names for
/// the public billing-preference operation `operation` — see
/// [`PREFERENCES_BACKEND_SUFFIX`]. Stated once, so both preference handlers
/// report the token their own backend operation carries.
pub fn preferences_backend_operation(operation: &str) -> String {
    format!("{operation}{PREFERENCES_BACKEND_SUFFIX}")
}

/// The `AccessDeniedException` payload for a `BillingFeature` this account's
/// caller has no access to.
///
/// Built from the account, the operation and the feature the request named —
/// the three things the decision was made from — so a client can tell "this
/// feature is not accessible to this caller" from "this feature is accessible
/// and has no preferences stored". `operation` is the PUBLIC name of the
/// preference operation the request called; the segment reports the backend
/// token that name resolves to ([`preferences_backend_operation`]), which is the
/// only discriminator this error carries, since the model gives it no member
/// besides `message`.
pub fn feature_access_denied(operation: &str, feature: &str) -> AccessDeniedException {
    AccessDeniedException {
        message: format!(
            "[Exception=AccessDeniedException][AccountId = {}][Operation = {}]\
             [Message=No access to feature {feature}]",
            caller_account_id(),
            preferences_backend_operation(operation)
        ),
    }
}

/// The tag set one request asks for, keyed by tag key — or the message that
/// refuses a list carrying the same key twice.
///
/// A tag set is keyed by key, so one request may not carry a key twice: the two
/// entries would decide the stored value by arrival order, and collecting them
/// into the keyed set would silently drop one of the two values. The modelled
/// `ResourceTagList` carries no `@uniqueItems`, so duplicates reach a handler
/// and the rule is the service's to apply. Every writer that takes a
/// `ResourceTagList` collects it through here, so the creator and the updater of
/// a view's tag set answer identical input identically. The caller renders the
/// `Err` as its own modeled `ValidationException` over the `resourceTags`
/// member, and writes nothing.
pub fn requested_tag_set(tags: &[ResourceTag]) -> Result<BTreeMap<String, String>, String> {
    let mut requested: BTreeMap<String, String> = BTreeMap::new();
    for tag in tags {
        if requested
            .insert(tag.key.clone(), tag.value.clone().unwrap_or_default())
            .is_some()
        {
            return Err(format!("the tag key {} is given more than once", tag.key));
        }
    }
    Ok(requested)
}

/// The message this service's ARN validation answers for a billing-view ARN
/// that names no view of this account: the requested ARN, in brackets, reported
/// as an invalid billing view ARN.
///
/// One form, stated once: `ListBillingViews` already refuses an `arns` filter
/// entry with it, and the billing-view reads that resolve a single `arn` answer
/// the same way.
pub fn invalid_billing_view_arn(arn: &str) -> String {
    format!("[{arn}] is an invalid billing view arn")
}

/// The `ValidationException` payload for an ARN that names no `CUSTOM` billing
/// view of this account — see [`ComAmazonawsBillingAwsbillingState::custom_view`].
///
/// `action` is the past participle of what the refused request would have done
/// to the view (`"edited"`, `"deleted"`), so the sentence names the action the
/// caller asked for rather than a fixed one.
///
/// This two-sentence wording belongs to the view-lifecycle writers that share
/// it; an operation whose own refusal text differs builds its own message
/// beside its handler rather than parameterising this one.
pub fn not_a_custom_view(arn: &str, action: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "The Billing View ARN does not correspond to a Custom Billing View. \
             Only Custom Billing Views may be {action}."
        ),
        reason: VALIDATION_REASON_FIELD.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: "arn".to_string(),
            message: format!("{arn} does not correspond to a Custom Billing View."),
        }]),
    }
}

/// The `BillingViewHealthStatusException` payload for a request against a view
/// whose stored status is not [`BILLING_VIEW_STATUS_HEALTHY`] — the modeled
/// error "thrown when a billing view's health status prevents an operation from
/// being performed … if the billing view is in a state other than HEALTHY".
///
/// `action` is the infinitive of what the refused request would have done to the
/// view (`"associate source views"`, `"be updated"`), so the sentence names the
/// action the caller asked for. Shared by every writer the model declares this
/// error for, so no two of them can disagree about the same gate or report it in
/// two wordings.
pub fn billing_view_not_healthy(
    arn: &str,
    status: &str,
    action: &str,
) -> BillingViewHealthStatusException {
    BillingViewHealthStatusException {
        message: format!(
            "Billing view {arn} is {status}; it must be \
             {BILLING_VIEW_STATUS_HEALTHY} to {action}"
        ),
    }
}

/// The ARN of the billing view with resource id `view_id`, in the modeled
/// `arn:aws[a-z-]*:(billing)::[0-9]{12}:billingview/…` form. Billing is a global
/// service, so the region segment is empty.
pub fn billing_view_arn(view_id: &str) -> String {
    ::engine::emulator_arn("billing", "", &format!("billingview/{view_id}"))
}

/// The opaque `nextToken` for a page that ended at `last`, where the page
/// boundary is a VALUE the store is keyed by (`ListBillingViews` walks
/// `billing_views`, a map keyed and ordered by ARN). A read whose page boundary
/// is a position in a list carries its own PAYLOAD through this same pair,
/// rather than a second codec — see `ops::list_source_views_for_billing_view`.
///
/// Hex keeps the token inside the modeled `PageToken` pattern
/// (`^[-a-zA-Z0-9+=/_]+$`) and length (1..4095) for every ARN the service mints,
/// and carries no meaning a client could construct one from.
pub fn encode_page_token(last: &str) -> String {
    ::hex::encode(last.as_bytes())
}

/// The value a `nextToken` resumes after, or `None` when the token is not one
/// [`encode_page_token`] produced.
///
/// The caller renders the `None` as its own modeled `ValidationException`, from
/// [`PAGE_TOKEN_REFUSAL`] and [`PAGE_TOKEN_FIELD_REFUSAL`] so the refusal reads
/// the same whichever read answered it.
pub fn decode_page_token(token: &str) -> Option<String> {
    ::hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// The `BillingViewHealthStatus` a read reports for `view`.
///
/// Both members are always present: the stored status code and the stored
/// reason list, which is EMPTY for a status that needs no explaining rather
/// than absent (see [`BillingView::health_status_reasons`]). Shared by every
/// read that projects a billing view, so `GetBillingView` and
/// `ListBillingViews` cannot report one view's health two ways.
pub fn health_status(view: &BillingView) -> BillingViewHealthStatus {
    BillingViewHealthStatus {
        status_code: Some(view.health_status_code.clone()),
        status_reasons: Some(view.health_status_reasons.clone()),
    }
}

/// A stored instant as the modeled `Timestamp` the wire carries.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs_f64(epoch_secs(at))
}

/// A stored instant as seconds since the epoch, for comparing against a
/// request's `Timestamp`.
pub fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// A request `Timestamp` as seconds since the epoch, on the same scale as
/// [`epoch_secs`].
pub fn date_time_secs(at: &::aws_smithy_types::DateTime) -> f64 {
    at.secs() as f64 + f64::from(at.subsec_nanos()) / 1_000_000_000.0
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBillingAwsbillingState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBillingAwsbillingState>)
}
