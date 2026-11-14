// SPDX-License-Identifier: CC0-1.0

//! The billing-view segment timeline `ListBillingViewSegments` reports.
//!
//! A segment is "a time range during which the billing domain and account
//! relationships for a billing view remained unchanged", so what the store
//! holds is a TIMELINE per billing view: one record per period of unchanged
//! relationships, each carrying the `BillingDomain` that applied and the
//! account relationships (organization management account, billing group
//! primary account, billing transfer account) that held during it. A mid-period
//! change is two records — the earlier one closed at the instant of the change,
//! the later one opened there — which is what makes a single request answer
//! with several segments, each with its own time range.
//!
//! The reader clips every reported range to the period the request asked about
//! and omits hidden records, so the segments a caller reads back can cover less
//! than the requested period.

use std::time::SystemTime;

use super::epoch_secs;

/// `BILLABLE`, the `BillingDomain` of data that "represents the final charges
/// owed to Amazon Web Services" (one of `BILLING_DOMAIN_VALUES` in `ops::types`).
pub const BILLING_DOMAIN_BILLABLE: &str = "BILLABLE";

/// `PRO_FORMA`, the `BillingDomain` of data "shaped by Billing Conductor that
/// doesn't reflect the final charges owed to Amazon Web Services".
pub const BILLING_DOMAIN_PRO_FORMA: &str = "PRO_FORMA";

/// One persisted billing-view segment: the period over which one billing
/// view's domain and account relationships did not change.
///
/// The members mirror the modeled `BillingViewSegmentsListElement`, with the
/// period's bounds kept as [`SystemTime`] so a stored segment and a requested
/// period compare on one scale ([`epoch_secs`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BillingViewSegment {
    /// The billing view whose timeline this segment belongs to, as
    /// `billing_views` keys it.
    pub billing_view_arn: String,
    /// The `BillingDomain` that applied over this period, e.g.
    /// [`BILLING_DOMAIN_BILLABLE`].
    pub domain: String,
    /// When the period began, inclusive. `None` for a segment that was already
    /// in effect before any period a caller can ask about, so a request's own
    /// lower bound is what the reader reports.
    pub begin_date_inclusive: Option<SystemTime>,
    /// When the period ended, exclusive. `None` for the segment still in
    /// effect, which no later record has closed.
    pub end_date_exclusive: Option<SystemTime>,
    /// The billing transfer account, reported "only when the caller is a
    /// billing transfer source account"; `None` for an account that is not one.
    pub billing_transfer_account_id: Option<String>,
    /// The organization's management account, reported "for organization member
    /// accounts"; `None` for an account in no organization.
    pub management_account_id: Option<String>,
    /// The billing group's primary account, reported "for billing group
    /// members"; `None` for an account in no billing group.
    pub billing_group_primary_account_id: Option<String>,
    /// Whether this segment is hidden from the caller. The documented response
    /// "omits hidden segments, so the segments it returns might not cover the
    /// entire requested time period", so a hidden record is kept in the
    /// timeline — it is part of the view's history — and left out of every
    /// answer.
    pub hidden: bool,
}

impl BillingViewSegment {
    /// The most segment records this emulator keeps resident.
    ///
    /// The timeline grows with every recorded relationship change, so it needs
    /// a ceiling for the same reason the view store does. An emulator resource
    /// bound, not a published AWS quota:
    /// [`super::ComAmazonawsBillingAwsbillingState::record_billing_view_segment`]
    /// refuses a write past it rather than growing without limit.
    pub const MAX_RESIDENT: usize = 1024;

    /// The segment in effect for `billing_view_arn` over a billing domain,
    /// bounded on neither side and hidden from nobody: the whole timeline of a
    /// view whose relationships have never changed.
    ///
    /// The account-relationship members are left absent, which is what a
    /// standalone account that belongs to no organization, no billing group and
    /// no billing transfer reports; a caller of this constructor fills the ones
    /// its account does have.
    pub fn in_effect(billing_view_arn: &str, domain: &str) -> Self {
        Self {
            billing_view_arn: billing_view_arn.to_string(),
            domain: domain.to_string(),
            begin_date_inclusive: None,
            end_date_exclusive: None,
            billing_transfer_account_id: None,
            management_account_id: None,
            billing_group_primary_account_id: None,
            hidden: false,
        }
    }

    /// The key this segment is stored under: its view's ARN, then the instant
    /// it began, left-padded to a fixed width so instants order numerically.
    ///
    /// The parts are joined with the ASCII unit separator (`0x1f`), which sorts
    /// below every character an ARN can carry, so ascending key order is one
    /// view's timeline oldest first, and the segments of one view occupy a
    /// contiguous range of the store ([`BillingViewSegment::arn_key_bounds`]).
    /// A segment with no lower bound began before every one that has one, and
    /// sorts first.
    pub fn key(&self) -> String {
        let began = self
            .begin_date_inclusive
            .map(|at| epoch_secs(at).max(0.0) as u64)
            .unwrap_or_default();
        format!("{}\u{1f}{began:0>20}", self.billing_view_arn)
    }

    /// The lowest [`BillingViewSegment::key`] of the billing view `arn`, and the
    /// lowest key above every key of that view — the inclusive lower and
    /// exclusive upper bounds of one view's timeline in the store.
    ///
    /// Every key of this view is the ARN followed by the key separator
    /// (`0x1f`), so the ARN itself sorts below them all and the ARN followed by
    /// a space (`0x20`) sorts above them all and below every key of a view whose
    /// ARN merely starts with this one.
    pub fn arn_key_bounds(arn: &str) -> (String, String) {
        (arn.to_string(), format!("{arn}\u{20}"))
    }

    /// Whether this segment's own period overlaps the half-open requested
    /// period `[start_secs, end_secs)`, both on [`epoch_secs`]' scale.
    ///
    /// An unbounded side never excludes a period: a segment that began before
    /// every queryable instant overlaps every period that has not already
    /// ended, and the segment still in effect overlaps every period that has
    /// begun.
    pub fn overlaps(&self, start_secs: f64, end_secs: f64) -> bool {
        self.begin_date_inclusive
            .is_none_or(|begin| epoch_secs(begin) < end_secs)
            && self
                .end_date_exclusive
                .is_none_or(|end| epoch_secs(end) > start_secs)
    }

    /// This segment's period clipped to the half-open requested period
    /// `[start_secs, end_secs)`: "each segment covers a portion of the requested
    /// time period", so neither reported bound ever falls outside what the
    /// caller asked about.
    pub fn clipped(&self, start_secs: f64, end_secs: f64) -> (f64, f64) {
        let begin = self
            .begin_date_inclusive
            .map(epoch_secs)
            .unwrap_or(start_secs)
            .max(start_secs);
        let end = self
            .end_date_exclusive
            .map(epoch_secs)
            .unwrap_or(end_secs)
            .min(end_secs);
        (begin, end)
    }
}

/// Seconds since the epoch as the modeled `Timestamp` the wire carries — the
/// bound of a clipped segment period, on [`epoch_secs`]' scale.
pub fn timestamp_at_secs(secs: f64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs_f64(secs)
}
