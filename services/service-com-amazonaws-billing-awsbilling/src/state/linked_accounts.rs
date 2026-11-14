// SPDX-License-Identifier: CC0-1.0

//! When each account of this emulator's consolidated billing family was linked
//! to the payer account and subscribed to Enterprise Support — the intervals
//! `ListEnterpriseSupportLinkedAccountCharges` reports and prorates an
//! account's Support-eligible spend by.
//!
//! The spend itself is NOT held here: it is the per-account-month record in
//! [`ComAmazonawsBillingAwsbillingState::support_eligible_spend`] (see the
//! `support_charges` module). What this module adds is WHEN the account was in
//! the Support profile, which is what turns a month's whole spend into the
//! prorated part of it the account was subscribed for.
//!
//! Empty on a fresh emulator: an account is linked and subscribed through AWS
//! Organizations and Support, neither of which this emulator's billing service
//! performs, so only a future operation that records a subscription — or this
//! service's test suite, through
//! [`ComAmazonawsBillingAwsbillingState::record_enterprise_support_linked_account`]
//! — writes it. An account with no record here is reported with the whole
//! billing period billable, because its bill for the month carries the
//! Enterprise Support plan; nothing synthesises an interval.

use std::time::{SystemTime, UNIX_EPOCH};

use super::ComAmazonawsBillingAwsbillingState;

/// The `accountType` reported for the payer account of the Support profile —
/// the account the model's `LinkedAccountCharge$payerAccountId` names as "the
/// payer account ID that is authorized to view Enterprise Support data for all
/// accounts in its Support profile".
///
/// The model documents no value list for `accountType` ("The type of
/// account."), so the two roles its own members distinguish — the payer account
/// and the linked accounts the report is broken down by — are what this
/// service reports, and a record that carries its own type reports that
/// instead.
pub const ACCOUNT_TYPE_PAYER: &str = "Payer";

/// The `accountType` reported for an account of the profile that is not its
/// payer account (see [`ACCOUNT_TYPE_PAYER`]).
pub const ACCOUNT_TYPE_LINKED: &str = "Linked";

/// An instant as whole seconds past the epoch, the scale a billing period's
/// length and an account's billable seconds are counted on.
///
/// Saturating rather than wrapping, so no stored instant can turn a duration
/// negative, and an instant before the epoch keeps its sign.
pub fn epoch_whole_secs(at: SystemTime) -> i64 {
    match at.duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_secs())
            .map_or(i64::MIN, i64::wrapping_neg),
    }
}

/// A half-open period an account was linked to the payer account, or subscribed
/// to Enterprise Support, as the modeled `EnterpriseSupportTimePeriod` reports
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportTimePeriod {
    /// The first instant of the period (`beginDate`).
    pub begin: SystemTime,
    /// The instant it ended (`endDate`); `None` is a period still in effect.
    pub end: Option<SystemTime>,
}

impl SupportTimePeriod {
    /// The period from `begin` on, still in effect.
    pub fn from(begin: SystemTime) -> Self {
        Self { begin, end: None }
    }

    /// The period from `begin` until `end`.
    pub fn between(begin: SystemTime, end: SystemTime) -> Self {
        Self {
            begin,
            end: Some(end),
        }
    }

    /// The part of this period that falls inside the half-open window
    /// `start_secs..end_secs`, as whole seconds past the epoch, or `None` when
    /// the two do not overlap.
    ///
    /// A period still in effect runs to the end of the window, so an account
    /// that never unsubscribed is billable for the whole of it.
    pub fn clipped_to(&self, start_secs: i64, end_secs: i64) -> Option<(i64, i64)> {
        let begin = epoch_whole_secs(self.begin).max(start_secs);
        let end = self
            .end
            .map_or(end_secs, |end| epoch_whole_secs(end).min(end_secs));
        (begin < end).then_some((begin, end))
    }
}

/// One account of the Enterprise Support profile: when it was linked to the
/// payer account and when it was subscribed to Enterprise Support.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterpriseSupportLinkedAccount {
    /// The 12-digit account the periods are about — the key this record is
    /// stored under.
    pub account_id: String,
    /// The `accountType` to report for it, when the account carries one of its
    /// own; `None` reports the account's role in the profile
    /// ([`ACCOUNT_TYPE_PAYER`] or [`ACCOUNT_TYPE_LINKED`]).
    pub account_type: Option<String>,
    /// The periods the account was linked to the payer account for.
    pub linked_periods: Vec<SupportTimePeriod>,
    /// The periods it was subscribed to Enterprise Support for — what a billing
    /// month's billable seconds are counted over.
    pub subscription_periods: Vec<SupportTimePeriod>,
}

impl EnterpriseSupportLinkedAccount {
    /// The most linked-account records this emulator keeps resident.
    ///
    /// The store grows with every account recorded, so it needs a ceiling for
    /// the same reason `support_eligible_spend` does. An emulator resource
    /// bound, not a published AWS quota.
    pub const MAX_RESIDENT: usize = 1024;

    /// The most periods either list may hold: the length the modeled
    /// `TimePeriodList` carries (`len ..500`), so no stored record can make a
    /// response break the model's own bound.
    pub const MAX_PERIODS: usize = 500;

    /// The record of an account with no period of its own recorded.
    pub fn new(account_id: &str) -> Self {
        Self {
            account_id: account_id.to_string(),
            account_type: None,
            linked_periods: Vec::new(),
            subscription_periods: Vec::new(),
        }
    }

    /// The same record with `periods` as the account's linkage periods.
    pub fn linked_for(mut self, periods: Vec<SupportTimePeriod>) -> Self {
        self.linked_periods = periods;
        self
    }

    /// The same record with `periods` as the account's Enterprise Support
    /// subscription periods.
    pub fn subscribed_for(mut self, periods: Vec<SupportTimePeriod>) -> Self {
        self.subscription_periods = periods;
        self
    }

    /// The parts of the account's linkage periods that fall inside the
    /// half-open window `start_secs..end_secs`, in the order they are stored.
    ///
    /// `None` when the account records no linkage period at all — there is
    /// nothing to report, as distinct from a recorded linkage that does not
    /// reach the window, which reports no period.
    pub fn linked_within(&self, start_secs: i64, end_secs: i64) -> Option<Vec<(i64, i64)>> {
        clip_all(&self.linked_periods, start_secs, end_secs)
    }

    /// The parts of the account's subscription periods that fall inside the
    /// half-open window `start_secs..end_secs`, read the way
    /// [`Self::linked_within`] reads the linkage periods.
    pub fn subscribed_within(&self, start_secs: i64, end_secs: i64) -> Option<Vec<(i64, i64)>> {
        clip_all(&self.subscription_periods, start_secs, end_secs)
    }
}

/// The parts of `periods` inside the half-open window `start_secs..end_secs`,
/// or `None` when `periods` is empty.
fn clip_all(
    periods: &[SupportTimePeriod],
    start_secs: i64,
    end_secs: i64,
) -> Option<Vec<(i64, i64)>> {
    if periods.is_empty() {
        return None;
    }
    Some(
        periods
            .iter()
            .filter_map(|period| period.clipped_to(start_secs, end_secs))
            .collect(),
    )
}

/// The whole seconds the windows in `clipped` cover.
///
/// Saturating, so no set of stored periods can overflow the count.
pub fn covered_seconds(clipped: &[(i64, i64)]) -> i64 {
    clipped.iter().fold(0_i64, |total, (begin, end)| {
        total.saturating_add(end.saturating_sub(*begin))
    })
}

impl ComAmazonawsBillingAwsbillingState {
    /// Persist one account's linkage and Enterprise Support subscription
    /// periods under its account id, replacing the record for the same account
    /// rather than duplicating it.
    ///
    /// Answers `false` without writing when the store is already at
    /// [`EnterpriseSupportLinkedAccount::MAX_RESIDENT`], or when either list is
    /// longer than [`EnterpriseSupportLinkedAccount::MAX_PERIODS`], so the
    /// caller refuses rather than growing the store without a bound or storing
    /// a record a response cannot report.
    pub fn record_enterprise_support_linked_account(
        &mut self,
        account: EnterpriseSupportLinkedAccount,
    ) -> bool {
        if account.linked_periods.len() > EnterpriseSupportLinkedAccount::MAX_PERIODS
            || account.subscription_periods.len() > EnterpriseSupportLinkedAccount::MAX_PERIODS
        {
            return false;
        }
        if !self
            .enterprise_support_linked_accounts
            .contains_key(&account.account_id)
            && self.enterprise_support_linked_accounts.len()
                >= EnterpriseSupportLinkedAccount::MAX_RESIDENT
        {
            return false;
        }
        self.enterprise_support_linked_accounts
            .insert(account.account_id.clone(), account);
        true
    }

    /// The linkage and subscription periods recorded for `account_id`, when any
    /// are.
    pub fn enterprise_support_linked_account(
        &self,
        account_id: &str,
    ) -> Option<&EnterpriseSupportLinkedAccount> {
        self.enterprise_support_linked_accounts.get(account_id)
    }

    /// The `accountType` to report for `account_id`: the type its own record
    /// carries, or else its role in the profile — [`ACCOUNT_TYPE_PAYER`] for
    /// the payer account the profile is billed to, [`ACCOUNT_TYPE_LINKED`] for
    /// every other account of it.
    pub fn enterprise_support_account_type(&self, account_id: &str) -> String {
        if let Some(account_type) = self
            .enterprise_support_linked_account(account_id)
            .and_then(|account| account.account_type.as_deref())
        {
            return account_type.to_string();
        }
        if self.is_billing_family_management_account(account_id) {
            ACCOUNT_TYPE_PAYER.to_string()
        } else {
            ACCOUNT_TYPE_LINKED.to_string()
        }
    }
}
