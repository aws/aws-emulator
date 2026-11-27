// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-billing-awsbilling service.
//!
//! The service's primary resource is the **billing view**. Every one of the five
//! implemented lifecycle operations (Create/Get/List/Update/DeleteBillingView)
//! computes its answer from [`ComAmazonawsBillingAwsbillingState::billing_views`],
//! which is keyed by the view's ARN and ordered, so a list and its pagination are
//! deterministic. Each view also has a **segment timeline** in
//! [`ComAmazonawsBillingAwsbillingState::billing_view_segments`] — the periods
//! over which its billing domain and account relationships did not change,
//! which `ListBillingViewSegments` reports; see the `billing_view_segments`
//! module below. The account relationships those segments report are NOT
//! this service's facts — they belong to AWS Organizations and to Billing
//! Conductor — so they are read off those services' stores on every read, which
//! is what makes a relationship change answer with several segments; see the
//! `account_relationships` module. What a read reports as a view's **health** is
//! not stored beside the view but derived, on every read, from the state that
//! view depends on — see the `health` module below.
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
//! credits `GetCredits` reports, keyed and ordered by `creditId` — the ledger
//! `RedeemCredits` mints a credit into when it redeems a promotional credit code —
//! and [`ComAmazonawsBillingAwsbillingState::credit_allocations`] the credits
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

use service_com_amazonaws_organizations_awsorganizationsv20161128::state as organizations_state;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;

/// The secret a store's continuation tokens are authenticated under — the
/// workspace's one reading of "a token this service ISSUED" (see
/// [`encode_page_token`]), so this crate adds no second mechanism for it.
pub use ::service_common::page_tokens::PageTokenKey;

use crate::ops::types::{
    AccessDeniedException, BillingViewHealthStatusException, Expression, ResourceNotFoundException,
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

/// The billing-view segment timeline `ListBillingViewSegments` reports.
mod billing_view_segments;

pub use billing_view_segments::*;

/// The account relationships those segments report, read off the stores of the
/// services that own them.
mod account_relationships;

pub use account_relationships::*;

/// The decision the tagging operations share about the resource an ARN names.
mod tagging;

pub use tagging::*;

/// The resource-based policy document a shared billing view carries, which
/// `GetResourcePolicy` reports.
mod resource_policy;

pub use resource_policy::*;

/// When each account of the billing family was linked and subscribed to
/// Enterprise Support, which `ListEnterpriseSupportLinkedAccountCharges`
/// prorates an account's spend by.
mod linked_accounts;

pub use linked_accounts::*;

/// The health a billing view reports, derived from the standing of its owner and
/// from the views it draws from.
mod health;

pub use health::*;

/// The page size every paginated read of this service holds its answer to.
mod page_size;

pub use page_size::*;

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

/// The fewest source views one billing view can draw from.
///
/// The other side of the very bound [`MAX_SOURCE_VIEWS_PER_VIEW`] names, read
/// off the same modelled trait: `BillingViewSourceViewsList` — the shape
/// `CreateBillingView$sourceViews`, `AssociateSourceViews$sourceViews`,
/// `DisassociateSourceViews$sourceViews` and the `@required`
/// `ListSourceViewsForBillingView$sourceViews` output all target — is `@length`
/// 1..10. A view is therefore born drawing from at least one view, and a
/// caller's write of the set it owns may not take that set below the minimum any
/// more than it may take it above the maximum: see [`remove_source_views`],
/// which every narrowing writer goes through.
pub const MIN_SOURCE_VIEWS_PER_VIEW: usize = 1;

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
/// [`unresolved_billing_view_arn`]. The ARN is still reported where the model has a
/// member for it, `ResourceNotFoundException$resourceId`.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The `message` that refuses a tagging request against a view AWS manages.
///
/// Shared by the tagging operations so the one rule — a managed view's tags are
/// the service's own, see [`BillingView::is_managed`] — reads the same whichever
/// of them answered.
pub const MANAGED_VIEW_NOT_TAGGABLE: &str = "Managed billing views cannot be tagged";

/// The `message` an operation reserved to the organization's management account
/// refuses any other caller with.
///
/// FIXED text for this error class: the sentence pair interpolates nothing — no
/// account id, no resource name, no ARN and no value from the request — so the
/// refusal reads the same whichever caller it answers and whichever operation
/// made the decision. Shared by every gate that consults
/// [`is_organization_management_account`], so a caller cannot learn from the
/// wording which account the service thinks it is.
pub const NOT_ORGANIZATION_MANAGEMENT_ACCOUNT: &str =
    "User account is not an AWS Organization's management account. \
     This operation is only permitted to management accounts.";

/// The most credits this emulator keeps resident for the whole billing family.
///
/// `credits` is the store `GetCredits` reports from; like `billing_views` it
/// needs a ceiling so that nothing can grow the process without bound. This is
/// an emulator resource bound, not a published AWS quota:
/// [`ComAmazonawsBillingAwsbillingState::insert_credit`] refuses past it.
pub const MAX_CREDITS: usize = 1024;

/// The one `CreditSharingType` value under which a credit's sharing scope is
/// controlled by a Cost Category rule — the condition
/// [`Credit::sharing_scope_is_cost_category_rule`] answers.
pub const CREDIT_SHARING_TYPE_COST_CATEGORY_RULE: &str = "COST_CATEGORY_RULE";

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

/// Why an account may not set a `CREDIT_PREFERENCE_OPTIONS` preference of one
/// credit — the answer
/// [`ComAmazonawsBillingAwsbillingState::credit_preference_authority`] gives for
/// a credit the caller cannot speak for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreditPreferenceRefusal {
    /// The consolidated billing family holds no credit under that id: either
    /// nothing is stored under it, or what is belongs to an account outside the
    /// family. The two are ONE refusal, so a caller that may speak for neither
    /// does not learn which it was.
    OutsideFamily,
    /// The credit belongs to another account of the same family, and the caller
    /// is not the family's management account: only the payer account speaks for
    /// a credit it does not own.
    NotOwned,
}

/// The suffix the model's own enum uses for a read-only history feature:
/// `BILLING_FEATURE_VALUES` pairs `RI_SHARING` with `RI_SHARING_HISTORY` and
/// `CREDIT_SHARING` with `CREDIT_SHARING_HISTORY`, and the
/// `UpdateBillingPreferences` `feature` doc says those two "are read-only and
/// cannot be updated".
pub const HISTORY_FEATURE_SUFFIX: &str = "_HISTORY";

/// The length of an `AccountId`, whose modelled pattern is `^[0-9]{12}$`.
const ACCOUNT_ID_LEN: usize = 12;

/// The longest a `CreditId` may be: the modeled shape is `@length(min: 1, max:
/// 32)` over `^[0-9]+$` (`smithy_query(query="shape", shape="CreditId")`), and
/// [`Credit::credit_id`] is the key a credit is stored under.
const CREDIT_ID_MAX_LEN: usize = 32;

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
/// read so none of them can go stale. Neither is the resource-based policy
/// `GetResourcePolicy` reports: the account attaches one by SHARING the view,
/// which is resource sharing's action, so the document is derived from the
/// shares that produce it ([`attached_resource_policy`]).
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
    /// A `BillingViewStatus` value, e.g. [`BILLING_VIEW_STATUS_HEALTHY`] — the
    /// condition the service RECORDED on this view itself, which is not the
    /// whole of the health a read reports: see [`BillingView::recorded_health`]
    /// and [`ComAmazonawsBillingAwsbillingState::view_health`], which derive the
    /// rest from the standing of the view's owner and from the views it draws
    /// from.
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
    /// The Cost Category whose rule controls the credit's sharing scope, read
    /// only under [`CREDIT_SHARING_TYPE_COST_CATEGORY_RULE`] — the one sharing
    /// type the model reports it for (see
    /// [`Credit::sharing_scope_is_cost_category_rule`]).
    pub cost_category_arn: Option<String>,
    /// The rule within that Cost Category, read under the same one condition.
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

    /// Whether this credit's sharing scope is the one a Cost Category rule
    /// controls — its `credit_sharing_type` being
    /// [`CREDIT_SHARING_TYPE_COST_CATEGORY_RULE`].
    ///
    /// The condition the modeled `CreditData$costCategoryArn` and
    /// `CreditData$ruleName` are reported under: both are documented "Present
    /// only when `creditSharingType` is `COST_CATEGORY_RULE`", so a record that
    /// carries either under any other sharing type (or under none at all) is
    /// reported without them.
    pub fn sharing_scope_is_cost_category_rule(&self) -> bool {
        self.credit_sharing_type.as_deref() == Some(CREDIT_SHARING_TYPE_COST_CATEGORY_RULE)
    }
}

/// Per-service state for com-amazonaws-billing-awsbilling. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsBillingAwsbillingState {
    /// Every persisted billing view, keyed and ordered by ARN. Bounded by
    /// [`MAX_BILLING_VIEWS`].
    pub billing_views: BTreeMap<String, BillingView>,
    /// Every billing view's segment timeline, keyed and ordered by
    /// [`BillingViewSegment::key`] — the view's ARN, then the instant the
    /// segment began — so one view's segments are a contiguous range walked
    /// oldest first and `ListBillingViewSegments`' page boundaries are stable.
    /// Bounded by [`BillingViewSegment::MAX_RESIDENT`]: a view's whole range is
    /// reclaimed by [`Self::forget_view`] when that view is deleted.
    ///
    /// Seeded with the one open-ended segment of the account's default PRIMARY
    /// view: its own recorded history is one period, bounded on neither side.
    /// What a record states is not all a read reports: the account
    /// relationships a segment carries belong to the services that own them, so
    /// a change recorded THERE splits a recorded period into several reported
    /// segments ([`reported_segments`]), and a record states a relationship only
    /// where this service recorded one itself.
    pub billing_view_segments: BTreeMap<String, BillingViewSegment>,
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
    /// [`organization_management_account_id`], which answers `None` while no
    /// organization has been created).
    pub billing_family_management_account_id: String,
    /// Every credit held by an account of this emulator's consolidated billing
    /// family, keyed and ordered by `creditId`, so `GetCredits` answers in a
    /// deterministic order. Bounded by [`MAX_CREDITS`].
    pub credits: BTreeMap<String, Credit>,
    /// Source of the next `creditId` [`Self::mint_credit_id`] hands out — the id
    /// a credit `RedeemCredits` mints is stored under in `credits`.
    pub next_credit_id: u64,
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
    /// The secret this store's `nextToken`s are authenticated under, drawn from
    /// the system's randomness when the store is created: it is what lets a
    /// paginated read tell a token it ISSUED from one a caller assembled (see
    /// [`encode_page_token`], reached through [`Self::page_token_key`]). Never
    /// reported and never logged — its own `Debug` renders nothing of it, so
    /// this state's derived one carries nothing of it either.
    page_token_key: PageTokenKey,
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
        };
        // The default view's domain is the account's own final charges, and the
        // record states no account relationship: which organization the account
        // belongs to and which billing group shapes its data are the owning
        // services' facts, read on every read rather than frozen here (see the
        // `account_relationships` module), so this one record is the view's
        // whole recorded history and is bounded on neither side.
        let primary_segment =
            BillingViewSegment::in_effect(&primary.arn, BILLING_DOMAIN_BILLABLE);
        let mut billing_view_segments = BTreeMap::new();
        billing_view_segments.insert(primary_segment.key(), primary_segment);
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
            billing_view_segments,
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
            // A fresh account holds no credits: the modeled `GetCredits`
            // response "returns an empty list when no credits exist".
            credits: BTreeMap::new(),
            // The modeled `CreditId` admits `^[0-9]+$`, so the first id minted
            // is the first positive number.
            next_credit_id: 1,
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
            // Fresh randomness per store: a token this store minted resumes a
            // walk, and one another store (another process, a restarted one)
            // minted does not.
            page_token_key: PageTokenKey::new(),
        }
    }
}

impl ComAmazonawsBillingAwsbillingState {
    /// The secret this store's `nextToken`s are authenticated under.
    ///
    /// Handed out by value — [`PageTokenKey`] is `Copy` — so a read takes the
    /// key off its store and goes on to borrow the records it is about to
    /// report in the same call. Never reported and never logged.
    pub fn page_token_key(&self) -> PageTokenKey {
        self.page_token_key
    }
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
    /// record, every idempotency token that named it, its whole segment timeline
    /// — the records [`BillingViewSegment::key`] keys by that very ARN — and its
    /// place in every other view's source-view set.
    ///
    /// Those four are all a billing view is referenced by, so afterwards no
    /// stored record names a view this store no longer holds: a create/delete
    /// loop reclaims every token and every segment slot it allocated instead of
    /// leaving entries behind that no API can observe or free (an ARN is never
    /// minted twice, so a record left under a deleted one would hold its place
    /// against [`MAX_IDEMPOTENCY_TOKENS`] or
    /// [`BillingViewSegment::MAX_RESIDENT`] for good), and no read can report an
    /// association that points at nothing — the state `AssociateSourceViews`
    /// refuses to write.
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
        // The deleted view's timeline is a contiguous key range of the segment
        // store, the one [`Self::billing_view_segments_of`] walks, so the whole
        // of it goes with the view: a segment record names the view it belongs
        // to, and this store no longer holds that view.
        let timeline: Vec<String> = self
            .billing_view_segments_of(arn)
            .map(|(key, _)| key.clone())
            .collect();
        for key in timeline {
            self.billing_view_segments.remove(&key);
        }
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

    /// Persist one billing-view segment under its [`BillingViewSegment::key`],
    /// replacing a record of the same view that began at the same instant
    /// rather than duplicating it.
    ///
    /// Answers `false` without writing when the timeline is already at
    /// [`BillingViewSegment::MAX_RESIDENT`], so the caller refuses rather than
    /// growing the store without a bound.
    pub fn record_billing_view_segment(&mut self, segment: BillingViewSegment) -> bool {
        let key = segment.key();
        if !self.billing_view_segments.contains_key(&key)
            && self.billing_view_segments.len() >= BillingViewSegment::MAX_RESIDENT
        {
            return false;
        }
        self.billing_view_segments.insert(key, segment);
        true
    }

    /// The segments of the billing view `arn`, oldest first, each beside the
    /// key it is stored under — the page boundary a `nextToken` resumes after.
    ///
    /// A view with no recorded timeline has no segments, which is the empty
    /// walk rather than a missing resource: whether the view EXISTS is decided
    /// from `billing_views`.
    pub fn billing_view_segments_of(
        &self,
        arn: &str,
    ) -> impl Iterator<Item = (&String, &BillingViewSegment)> {
        let (low, high) = BillingViewSegment::arn_key_bounds(arn);
        self.billing_view_segments.range(low..high)
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

    /// The first entry of `sources` that names no billing view this store
    /// holds, or `None` when every one of them resolves.
    ///
    /// A billing view draws its data from OTHER billing views, so a
    /// `sourceViews` entry under which no view is stored names nothing to draw
    /// from. Every writer of a view's source list — the create that mints a view
    /// over them and the association that adds one later — refuses such a
    /// request with the modeled `ResourceNotFoundException`, built by
    /// [`billing_view_not_found`], so no read can report a source the same
    /// store answers 404 for.
    pub fn unknown_source_view<'a>(&self, sources: &'a [String]) -> Option<&'a str> {
        sources
            .iter()
            .find(|source| !self.billing_views.contains_key(*source))
            .map(String::as_str)
    }

    /// Whether another view fits under [`MAX_BILLING_VIEWS`], counting the
    /// idempotency entry it would write against [`MAX_IDEMPOTENCY_TOKENS`].
    pub fn has_room_for_another_view(&self, writes_token: bool) -> bool {
        self.billing_views.len() < MAX_BILLING_VIEWS
            && (!writes_token || self.idempotency_tokens.len() < MAX_IDEMPOTENCY_TOKENS)
    }

    /// Whether `account_id` is the management (payer) account of the
    /// consolidated billing family — the only identity that may set a sharing
    /// preference. Distinct from [`is_organization_management_account`]: a
    /// standalone account is its own family's payer account but nobody's
    /// organization management account.
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

    /// Whether `account_id` may set a `CREDIT_PREFERENCE_OPTIONS` preference of
    /// the credit `credit_id`, and why not when it may not.
    ///
    /// A `credit/{creditId}/{option}` key identifies ONE credit, and the
    /// `UpdateBillingPreferences` doc scopes it to that credit's ownership: the
    /// key is settable "by member accounts for credits they own, or by the
    /// management account for any credit in the family". So the id is resolved
    /// against [`Self::credits`], the store keyed by exactly that id: an id no
    /// credit of this family is held under names nothing the caller can speak
    /// for, whichever option the key carries.
    pub fn credit_preference_authority(
        &self,
        account_id: &str,
        credit_id: &str,
    ) -> Result<(), CreditPreferenceRefusal> {
        let Some(credit) = self.credits.get(credit_id) else {
            return Err(CreditPreferenceRefusal::OutsideFamily);
        };
        if credit.account_id == account_id {
            return Ok(());
        }
        if self.family_account_name(&credit.account_id).is_none() {
            return Err(CreditPreferenceRefusal::OutsideFamily);
        }
        if self.is_billing_family_management_account(account_id) {
            return Ok(());
        }
        Err(CreditPreferenceRefusal::NotOwned)
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

    /// The stored view `arn` names when it is one this account may edit or
    /// delete: an existing `CUSTOM` view.
    ///
    /// `None` is the answer for both an ARN no view is stored under and one
    /// naming a stored view that is not `CUSTOM` (the account's `PRIMARY`
    /// view), so this says only whether the view is eligible. An operation
    /// whose model declares `ResourceNotFoundException` separates the two,
    /// checking `billing_views` for the ARN first and answering
    /// [`unresolved_billing_view_arn`] when it holds nothing there; one that
    /// declares no such error (`DeleteBillingView`) has only the eligibility
    /// refusal to make.
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

    /// Whether another credit fits in the store, i.e. whether
    /// [`Self::insert_credit`] would store one rather than refuse it. Read-only,
    /// so a handler can refuse before it consumes anything the credit would be
    /// minted from.
    pub fn has_room_for_a_credit(&self) -> bool {
        self.credits.len() < MAX_CREDITS
    }

    /// Mint a `creditId` no stored credit is using, and advance the id source.
    ///
    /// The modeled `CreditId` is 1..32 characters of `^[0-9]+$`, so an id is a
    /// decimal number; `GetCreditAllocationHistory$creditId` carries the same id
    /// as a `long`, which is what keeps it numeric rather than opaque. The
    /// returned id is not stored by this call — the caller inserts the credit it
    /// belongs to with [`Self::insert_credit`].
    pub fn mint_credit_id(&mut self) -> String {
        loop {
            let credit_id = self.next_credit_id.to_string();
            self.next_credit_id += 1;
            if !self.credits.contains_key(&credit_id) {
                return credit_id;
            }
        }
    }

    /// The credits `GetCredits` reports, in `creditId` order.
    ///
    /// `account_id` selects the owning account. `family_payer` widens the
    /// selection: `Some(payer)` reports the credits of every account whose bill
    /// `payer` pays — its own, and those of the MEMBERS of the consolidated
    /// billing family it is the management account of — through
    /// [`Self::history_covers_billed_account`], the family traversal every
    /// billing history in this service reads by, so a credit owned by an
    /// account OUTSIDE the family belongs to no answer. The decision to widen
    /// belongs to the caller, which consults
    /// [`Self::is_billing_family_management_account`]. Both bounds of the
    /// requested period are inclusive, on [`epoch_secs`]' scale, and a credit is
    /// reported when its own life overlaps it.
    ///
    /// A pure read: it answers from the store and changes nothing in it.
    pub fn credits_for_period(
        &self,
        account_id: &str,
        family_payer: Option<&str>,
        start_secs: f64,
        end_secs: f64,
    ) -> Vec<Credit> {
        self.credits
            .values()
            .filter(|credit| match family_payer {
                Some(payer) => self.history_covers_billed_account(payer, &credit.account_id),
                None => credit.account_id == account_id,
            })
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
///
/// The id segment IDENTIFIES a credit, so it has to be able to name one: only a
/// modelled `CreditId` ([`is_credit_id`]) is read as one. A key whose id segment
/// is outside that shape names no credit at all and is not a key of this
/// feature.
pub fn credit_key_parts(key: &str) -> Option<(&str, &str)> {
    let mut segments = key.split('/');
    let prefix = segments.next()?;
    let credit_id = segments.next()?;
    let option = segments.next()?;
    let extra = segments.next();
    if prefix != CREDIT_KEY_PREFIX
        || !is_credit_id(credit_id)
        || option.is_empty()
        || extra.is_some()
    {
        return None;
    }
    Some((credit_id, option))
}

/// Whether `value` is a `CreditId`: 1..[`CREDIT_ID_MAX_LEN`] characters of
/// `^[0-9]+$`, the modeled shape of the id a credit is stored under.
pub fn is_credit_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= CREDIT_ID_MAX_LEN
        && value.bytes().all(|byte| byte.is_ascii_digit())
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

/// The management account of the AWS Organizations organization this emulated
/// account belongs to, or `None` while it belongs to none.
///
/// An organization is AWS Organizations' own resource, so the fact is read from
/// THAT service's store, at the instant the decision is made — the same way the
/// principal a refusal names is read from STS's ([`caller_identity_arn`]).
/// Nothing is cached here: an account becomes a management account by creating
/// an organization through Organizations, and that call is the only thing that
/// can make this answer an account id.
///
/// `None` also when this service is mounted on an engine that holds no
/// Organizations store: no organization is recorded anywhere such a process can
/// see, so the account is standalone.
pub fn organization_management_account_id<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<String> {
    let organizations = organizations_state::service_state(engine)?;
    let organizations = organizations.lock();
    organizations
        .organization
        .as_ref()
        .map(|organization| organization.master_account_id.clone())
}

/// Whether `account_id` is the management account of the organization this
/// account belongs to — the gate an operation reserved to that identity
/// consults, refusing every other caller with
/// [`NOT_ORGANIZATION_MANAGEMENT_ACCOUNT`].
///
/// `false` while no organization exists: a standalone account is the payer
/// account of its own consolidated billing family (see
/// [`ComAmazonawsBillingAwsbillingState::is_billing_family_management_account`])
/// but is nobody's organization management account.
pub fn is_organization_management_account<Eng: Send + Sync + 'static>(
    engine: &Eng,
    account_id: &str,
) -> bool {
    organization_management_account_id(engine).as_deref() == Some(account_id)
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

/// Add the source views one request names to the ones `held` already draws
/// from, as a SET: the first mention of an ARN is kept, in the order it arrives,
/// and every later mention of an ARN already in the list adds nothing.
///
/// A view's source list is the SET of billing views it draws its data from:
/// `BillingViewElement$sourceViewCount` is "the number of source views
/// associated with this billing view", and one view is associated with another
/// once however many times a request names it. The modelled
/// `BillingViewSourceViewsList` carries no `@uniqueItems`, so duplicates reach a
/// handler and the rule is the service's to apply. Every writer of
/// [`BillingView::source_views`] — the create that mints a view over its sources
/// and the association that adds one later — collects through here, so no stored
/// record holds the same source twice and nothing derived from it on read (the
/// count, the paged source listing, the per-view bound
/// [`MAX_SOURCE_VIEWS_PER_VIEW`]) reports one association as two.
pub fn add_source_views(held: &mut Vec<String>, requested: impl IntoIterator<Item = String>) {
    for source in requested {
        if !held.contains(&source) {
            held.push(source);
        }
    }
}

/// The source views `held` keeps once the ones `requested` names are removed, as
/// a SET — or the message that refuses a removal which would take the stored set
/// below [`MIN_SOURCE_VIEWS_PER_VIEW`].
///
/// The inverse of [`add_source_views`], and bounded like it: the association is a
/// set, so what the view keeps is the set difference (a source named twice, or
/// one the view does not draw from, removes what it names once and nothing
/// else), and the result has to stay inside the modelled `@length` of
/// `BillingViewSourceViewsList` — a view that draws from nothing is a record no
/// create can produce and no read can report inside that `@required` 1..10
/// output. Every writer that NARROWS a view's source set collects through here,
/// so no two of them can disagree about the bound.
///
/// A request that removes NOTHING — every ARN it names is one the view does not
/// draw from — leaves the stored set exactly as it stands, so it is answered
/// rather than refused however small that set is: it moves the set no closer to
/// the bound, and these writers are `@idempotent`. That exemption is also what
/// keeps a view whose last source was DELETED out from under it (see
/// [`ComAmazonawsBillingAwsbillingState::forget_view`], the cascade a forced
/// delete runs) answerable: the caller of this did not take that set down, and
/// refusing it would not put the deleted source back.
///
/// The caller renders the `Err` as its own modeled `ValidationException` over
/// the member that carried the list, and writes nothing.
pub fn remove_source_views(held: &[String], requested: &[String]) -> Result<Vec<String>, String> {
    let remaining: Vec<String> = held
        .iter()
        .filter(|source| !requested.contains(source))
        .cloned()
        .collect();
    if remaining.len() < held.len() && remaining.len() < MIN_SOURCE_VIEWS_PER_VIEW {
        return Err(format!(
            "A billing view must draw from at least {MIN_SOURCE_VIEWS_PER_VIEW} source view"
        ));
    }
    Ok(remaining)
}

/// Check the relations between the members of one request's data filter
/// expression — or the message that refuses it.
///
/// `TimeRange` "specifies a time range with inclusive begin and end dates", so
/// a range whose inclusive begin falls later than its inclusive end selects no
/// instant at all: a period a caller cannot have meant, refused rather than
/// stored and handed back on every later read. Neither bound carries a trait
/// relating it to the other, so the boundary validator cannot see this and the
/// rule is the service's to apply — the same relation every other two-bound
/// date range a request to this service carries is refused on. A range that
/// omits one bound is open on that side and still selects instants, so only a
/// range carrying both bounds is related.
///
/// Every writer of [`BillingView::data_filter_expression`] checks through here,
/// so the creator and the updater of a view's data filter answer identical
/// input identically. The caller renders the `Err` as its own modeled
/// `ValidationException` over the member that carried the expression, and
/// writes nothing.
pub fn check_data_filter_expression(expression: &Expression) -> Result<(), &'static str> {
    let Some(range) = &expression.time_range else {
        return Ok(());
    };
    let (Some(begin), Some(end)) = (&range.begin_date_inclusive, &range.end_date_inclusive) else {
        return Ok(());
    };
    if date_time_secs(begin) > date_time_secs(end) {
        return Err(INVERTED_TIME_RANGE);
    }
    Ok(())
}

/// The sentence [`check_data_filter_expression`] refuses an inverted period
/// with, naming the two members whose relation fails — the wording this crate
/// already uses for the other range it refuses whose two bounds are both
/// inclusive (`activeTimeRange`), with this range's member names.
const INVERTED_TIME_RANGE: &str = "beginDateInclusive must not be later than endDateInclusive";

/// The message this service's ARN validation answers for a billing-view ARN
/// that names no view of this account: the requested ARN, in brackets, reported
/// as an invalid billing view ARN.
///
/// One form, stated once: `ListBillingViews` refuses an `arns` filter entry with
/// it, and every operation that resolves the view its own request identifies by
/// `arn` answers the same sentence, as the modeled `ResourceNotFoundException`
/// built by [`unresolved_billing_view_arn`] — the only place that payload is
/// assembled.
pub fn invalid_billing_view_arn(arn: &str) -> String {
    format!("[{arn}] is an invalid billing view arn")
}

/// The modeled `ResourceNotFoundException` payload (HTTP 404) for the billing
/// view a request identifies by its own `arn` member when this account holds no
/// view under that ARN — "The specified ARN in the request doesn't exist".
///
/// ONE form for that case, stated once, so an unresolvable ARN of the caller's
/// own account draws the same refusal whichever operation resolved it to act on
/// that view's own record: the message is this service's ARN-validation sentence
/// ([`invalid_billing_view_arn`]), and the modeled members report the ARN that
/// resolved to nothing and the billing-view resource type. The two operations
/// that EDIT or END the view's life answer [`not_a_custom_view`] for the same
/// ARN instead, deciding eligibility first. A billing-view ARN a
/// request names BESIDE that view — a source the view draws from — is a
/// different refusal, which names the member it arrived in
/// ([`billing_view_not_found`]).
pub fn unresolved_billing_view_arn(arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: invalid_billing_view_arn(arn),
        resource_id: arn.to_string(),
        resource_type: BILLING_VIEW_RESOURCE_TYPE.to_string(),
    }
}

/// The `ValidationException` payload for an ARN that names no `CUSTOM` billing
/// view of this account — see [`ComAmazonawsBillingAwsbillingState::custom_view`].
///
/// `action` is the past participle of what the refused request would have done
/// to the view (`"edited"`, `"deleted"`), so the sentence names the action the
/// caller asked for rather than a fixed one.
///
/// It answers for an ARN that resolves to a view of another type AND for one
/// that resolves to nothing at all, because the two operations that share it —
/// the edit and the delete — decide ELIGIBILITY before existence: what such a
/// caller asked for is a change only a custom view admits, so the refusal names
/// the views that admit it rather than reporting whether the ARN resolves. Every
/// other operation resolves the same member the other way round and answers
/// [`unresolved_billing_view_arn`] for an ARN nothing is stored under.
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

/// The modeled `ResourceNotFoundException` payload (HTTP 404) for a billing-view
/// ARN no stored view answers to, named by the request member that carried it.
///
/// Shared by every operation that resolves a billing-view ARN a request names
/// BESIDE the view it acts on —
/// the source views that view draws from (see
/// [`ComAmazonawsBillingAwsbillingState::unknown_source_view`]) — so the same
/// unresolvable source is refused in one wording whichever writer of the field
/// was asked, and the refusal says which member carried it. The view the
/// request identifies by its own `arn` is refused by
/// [`unresolved_billing_view_arn`] instead, in this service's ARN-validation
/// form.
pub fn billing_view_not_found(arn: &str, member: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("{member}: billing view {arn} does not exist"),
        resource_id: arn.to_string(),
        resource_type: BILLING_VIEW_RESOURCE_TYPE.to_string(),
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

/// Separates the read that minted a token from the payload that read resumes
/// after, inside the cursor the token authenticates.
///
/// NUL, which no page boundary any read of this service mints can carry — the
/// store keys are built from modeled `BillingMonth`, `AccountId`, `CreditId`,
/// service-name and ARN values joined by the unit separator, and a position is
/// decimal digits — so the two fields always read back whole and no payload can
/// spell another read's identity.
const PAGE_TOKEN_SCOPE_SEPARATOR: char = '\u{0}';

/// The opaque `nextToken` for a page of `minted_by` that ended at `last`, where
/// the page boundary is a VALUE the store is keyed by (`ListBillingViews` walks
/// `billing_views`, a map keyed and ordered by ARN). A read whose page boundary
/// takes more than one value — the sequence it walks beside the entries already
/// reported — carries its own PAYLOAD through this same pair, rather than a
/// second codec; see `ops::list_source_views_for_billing_view`.
///
/// `minted_by` is the name of the read that issued the token, and it travels
/// IN the token because a payload alone does not say which walk it is a
/// boundary of: every read of this service pages over its own sequence, so a
/// cut point one read issued names some other entry — or no entry at all — in
/// another's. [`decode_page_token`] takes a token back only for the read named
/// in it, so no read resumes a walk from a token it did not issue.
///
/// The cursor is AUTHENTICATED under `key`, the secret this store drew from the
/// system's randomness ([`ComAmazonawsBillingAwsbillingState::page_token_key`],
/// the workspace's one reading of that in
/// [`::service_common::page_tokens::PageTokenKey`]): the cursor stays readable
/// — a continuation token is opaque, not confidential — but no caller can
/// produce the authenticator for a cut point of its own, so a token this store
/// did not mint for this read is refused rather than honoured.
///
/// Hex keeps the token inside the modeled `PageToken` pattern
/// (`^[-a-zA-Z0-9+=/_]+$`) and length (1..4095) for every payload and read name
/// the service mints, and carries no meaning a client could construct one from.
pub fn encode_page_token(key: &PageTokenKey, minted_by: &str, last: &str) -> String {
    ::hex::encode(key.mint(&format!(
        "{minted_by}{PAGE_TOKEN_SCOPE_SEPARATOR}{last}"
    )))
}

/// The value a `nextToken` resumes after for the read named `read_by`, or
/// `None` when the token is not one [`encode_page_token`] produced FOR THAT
/// READ under `key`: one that is not hex, one too short to carry an
/// authenticator, one whose authenticator does not match the cursor it travels
/// with (a cut point a caller assembled, or one another store minted), and one
/// this store minted for another of its paginated reads.
///
/// The caller renders the `None` as its own modeled `ValidationException`, from
/// [`PAGE_TOKEN_REFUSAL`] and [`PAGE_TOKEN_FIELD_REFUSAL`] so the refusal reads
/// the same whichever read answered it.
pub fn decode_page_token(key: &PageTokenKey, read_by: &str, token: &str) -> Option<String> {
    let cursor = key.cursor_of(&::hex::decode(token).ok()?)?;
    let (minted_by, payload) = cursor.split_once(PAGE_TOKEN_SCOPE_SEPARATOR)?;
    (minted_by == read_by).then(|| payload.to_string())
}

/// Separates the fields a scoped token's payload carries: the values of the
/// request members that choose WHICH sequence the read walks, and then the
/// cursor inside that sequence.
///
/// `#` is the separator `ops::list_source_views_for_billing_view` already
/// carries its own scoped payload in, so the crate spells one payload one way.
/// No scope value can contain it: a scope value is a modeled `BillingViewArn`,
/// `BillingMonth` or `Timestamp` value, and none of those patterns admits `#`.
/// Only the scope fields are read back by position, so a cursor that spelled a
/// `#` would still read back whole — it is everything behind the last scope
/// field.
pub const PAGE_TOKEN_FIELD_SEPARATOR: char = '#';

/// The opaque `nextToken` of `minted_by` for the cut point `last` of the ONE
/// sequence `scope` selects.
///
/// A cursor is a position among the entries a request selected, so it is a cut
/// point of that sequence and of no other: every request member that decides
/// which entries the walk holds, or what each of them reports, travels in the
/// token beside the cursor, and [`decode_scoped_page_token`] hands the cursor
/// back only to a request that selected the same sequence. Each `scope` entry
/// pairs what the member names — the word the refusal of a token from another
/// sequence uses — with the value this request carries for it; `maxResults` is
/// deliberately NOT one of them, because a page size is how much of one walk a
/// caller takes rather than which entries the walk is over.
///
/// The transcoding, the binding to the read that minted the token and the
/// authentication under the store's key are [`encode_page_token`]'s, so this
/// crate keeps one page-token codec and one acceptance rule.
pub fn encode_scoped_page_token(
    key: &PageTokenKey,
    minted_by: &str,
    scope: &[(&str, &str)],
    last: &str,
) -> String {
    let mut payload = String::new();
    for (names, value) in scope {
        // A scope field is read back by POSITION, so a value that spelled the
        // separator would be read back as two fields and a token minted for the
        // right sequence would be refused as one of another. No modeled value a
        // read scopes by admits it; this is what says so where it is written.
        debug_assert!(
            !value.contains(PAGE_TOKEN_FIELD_SEPARATOR),
            "the {names} a page token is scoped by cannot spell \
             {PAGE_TOKEN_FIELD_SEPARATOR}"
        );
        payload.push_str(value);
        payload.push(PAGE_TOKEN_FIELD_SEPARATOR);
    }
    payload.push_str(last);
    encode_page_token(key, minted_by, &payload)
}

/// What a `nextToken` offered to a scoped paginated read names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopedPageToken<'scope> {
    /// A cut point in the very sequence the request selects: the value the walk
    /// resumes strictly after.
    Cursor(String),
    /// A cut point this read minted while walking ANOTHER sequence, which names
    /// some other entry of the requested one or no entry at all. Carries what
    /// the first differing scope member names, for the refusal's wording.
    OtherSequence(&'scope str),
    /// Not a token this read issued at all: one the store cannot read back, one
    /// a caller assembled, and one minted for another of this service's reads.
    Unusable,
}

/// The cut point a `nextToken` names for the read `read_by` walking the sequence
/// `scope` selects, as [`encode_scoped_page_token`] minted it.
///
/// A token whose scope disagrees with this request's names a cut point of
/// another sequence — of another resource's walk, or of another period — so it
/// is reported as [`ScopedPageToken::OtherSequence`] rather than compared
/// against the keys of the sequence being walked, a comparison that would
/// silently drop or repeat entries.
pub fn decode_scoped_page_token<'scope>(
    key: &PageTokenKey,
    read_by: &str,
    token: &str,
    scope: &'scope [(&str, &str)],
) -> ScopedPageToken<'scope> {
    let Some(payload) = decode_page_token(key, read_by, token) else {
        return ScopedPageToken::Unusable;
    };
    // The scope fields come first, one per member, and everything behind them is
    // the cursor.
    let mut fields = payload.splitn(scope.len() + 1, PAGE_TOKEN_FIELD_SEPARATOR);
    for (names, value) in scope {
        let Some(stored) = fields.next() else {
            return ScopedPageToken::Unusable;
        };
        if stored != *value {
            return ScopedPageToken::OtherSequence(names);
        }
    }
    match fields.next() {
        Some(cursor) => ScopedPageToken::Cursor(cursor.to_string()),
        // Every token this codec mints carries the cursor behind its scope, so a
        // payload with nothing behind it is not one it produced.
        None => ScopedPageToken::Unusable,
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

/// An instant as MILLISECONDS since the epoch, the scale on which this
/// emulator's promotional credit codes are issued and expire
/// (`RedeemCredits`). Saturating: an instant past what the scale can hold
/// reads as its maximum rather than wrapping.
pub fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// Milliseconds since the epoch as the instant this store holds: the inverse
/// of [`epoch_millis`], for an instant that arrives on that scale.
///
/// The scale carries no instant before the epoch — a promotional credit code
/// is issued and expires after it — so a negative figure is not one of this
/// store's instants and reads as the epoch itself. It SATURATES at both ends,
/// as [`epoch_millis`] does: a figure the platform's instant cannot be advanced
/// by reads as the epoch rather than ending the request, so no value a request
/// carries on this member can stop the operation serving it.
pub fn instant_from_millis(millis: i64) -> SystemTime {
    UNIX_EPOCH
        .checked_add(std::time::Duration::from_millis(
            u64::try_from(millis).unwrap_or(0),
        ))
        .unwrap_or(UNIX_EPOCH)
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
