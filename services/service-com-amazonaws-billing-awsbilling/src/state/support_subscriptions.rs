// SPDX-License-Identifier: CC0-1.0

//! The Support subscription contracts this service's persisted bills record —
//! what `ListBusinessSupportSubscriptionHistory` reports.
//!
//! A contract is NOT a store of its own. Which Support plan an account was
//! subscribed to in a billing month is already persisted, once, on that
//! account-month's bill
//! ([`SupportEligibleSpend::support_plan_name`][super::SupportEligibleSpend],
//! the plan `ListBusinessSupportAccountCharges` PRICES the month's spend by), so
//! a contract is read FROM those bills rather than kept beside them: a
//! subscription contract is the unbroken run of billing months whose bills
//! record one account on one plan, and the two reports therefore cannot
//! disagree about which plan an account was on.
//!
//! Empty on a fresh emulator, for the reason the `support_charges` module gives:
//! bills accrue from a billing run this emulator does not perform, so the
//! records come from a future operation that bills usage — or from this
//! service's test suite, through
//! [`ComAmazonawsBillingAwsbillingState::record_support_eligible_spend`].
//! Nothing here synthesises a contract.

use std::collections::BTreeMap;

use super::{month_index, month_period, parse_month, ComAmazonawsBillingAwsbillingState};

/// One account's unbroken run of billing months on one Support plan: the
/// subscription contract the modeled `BusinessSupportSubscriptionContract`
/// reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportSubscriptionContract {
    /// The 12-digit account the subscription is for.
    pub account_id: String,
    /// The Support plan the run's bills record, one of
    /// [`super::SUPPORT_PLAN_NAMES`].
    pub plan_name: String,
    /// The first billing month of the run, in `YYYY-MM` form.
    pub first_billing_month: String,
    /// The last billing month of the run, in `YYYY-MM` form. Equal to
    /// [`Self::first_billing_month`] for a one-month contract.
    pub last_billing_month: String,
}

impl SupportSubscriptionContract {
    /// The key a page of contracts resumes after: account, then the month the
    /// contract starts in.
    ///
    /// The parts are joined with the ASCII unit separator (`0x1F`) the spend
    /// store's keys use, which sorts below every character either part can
    /// carry, so ascending key order is ascending account order and, inside one
    /// account, ascending contract order.
    pub fn key(&self) -> String {
        format!("{}\u{1f}{}", self.account_id, self.first_billing_month)
    }

    /// The half-open UTC period the contract runs for: the first instant of its
    /// first billing month, and the first instant of the month after its last —
    /// the same reading of a period's two ends the charge reports' periods
    /// carry (see [`month_period`]).
    ///
    /// `None` only for a run whose months cannot be rendered as a modeled
    /// `Timestamp`.
    pub fn period(&self) -> Option<(::aws_smithy_types::DateTime, ::aws_smithy_types::DateTime)> {
        let (start, _) = month_period(&self.first_billing_month)?;
        let (_, end) = month_period(&self.last_billing_month)?;
        Some((start, end))
    }
}

impl ComAmazonawsBillingAwsbillingState {
    /// The Support subscription contracts the billing history reported for
    /// `account_id` covers, ascending by account and then by the month each
    /// contract starts in.
    ///
    /// Read off the persisted bills: for each account of the consolidated
    /// billing family whose bills belong to that history
    /// ([`Self::history_covers_billed_account`], the family traversal every
    /// billing history in this service reads by), consecutive billing months
    /// recording the SAME plan are one contract, and a month whose bill records
    /// another plan — or a month with no bill at all — ends it and starts the
    /// next.
    ///
    /// Only a plan the modeled `planName` documents is reported — its "Valid
    /// values" are `AWSSupportBusiness` (Business Support plan),
    /// `AWSSupportDeveloper` (Developer Support plan) and
    /// `AWSSupportEssential` (Basic Support plan), the three
    /// [`super::SUPPORT_PLAN_NAMES`] holds. A bill recording another plan, such as
    /// the Enterprise Support plan an Enterprise Support profile's accounts are
    /// billed under, is that profile's contract to report and not one of these.
    pub fn support_subscription_contracts(
        &self,
        account_id: &str,
    ) -> Vec<SupportSubscriptionContract> {
        // The bills of one account, ascending by the month they are for: the
        // run grouping below needs one account's whole timeline, which the
        // store's month-first key order interleaves across accounts.
        let mut months_by_account: BTreeMap<&str, Vec<(i32, &str, &str)>> = BTreeMap::new();
        for spend in self.support_eligible_spend.values() {
            if !self.history_covers_billed_account(account_id, &spend.account_id) {
                continue;
            }
            if !super::SUPPORT_PLAN_NAMES.contains(&spend.support_plan_name.as_str()) {
                continue;
            }
            let Some(index) = parse_month(&spend.billing_month).map(month_index) else {
                continue;
            };
            months_by_account
                .entry(spend.account_id.as_str())
                .or_default()
                .push((
                    index,
                    spend.billing_month.as_str(),
                    spend.support_plan_name.as_str(),
                ));
        }

        let mut contracts: Vec<SupportSubscriptionContract> = Vec::new();
        for (account, mut months) in months_by_account {
            months.sort_by_key(|(index, _, _)| *index);
            let mut run: Option<(i32, SupportSubscriptionContract)> = None;
            for (index, month, plan) in months {
                match run.take() {
                    // The month after the run's last one, on the same plan:
                    // the same contract, extended to it.
                    Some((previous_index, mut contract))
                        if contract.plan_name == plan && index == previous_index + 1 =>
                    {
                        contract.last_billing_month = month.to_string();
                        run = Some((index, contract));
                    }
                    previous => {
                        if let Some((_, contract)) = previous {
                            contracts.push(contract);
                        }
                        run = Some((
                            index,
                            SupportSubscriptionContract {
                                account_id: account.to_string(),
                                plan_name: plan.to_string(),
                                first_billing_month: month.to_string(),
                                last_billing_month: month.to_string(),
                            },
                        ));
                    }
                }
            }
            if let Some((_, contract)) = run {
                contracts.push(contract);
            }
        }
        contracts
    }
}
