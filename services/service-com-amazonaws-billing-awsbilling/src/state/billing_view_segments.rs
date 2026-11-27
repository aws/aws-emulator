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
//!
//! What a record states is not all a segment reports: the account relationships
//! are the facts of the services that own them, read off their stores on every
//! read (see `super::account_relationships`), so a recorded period is REFINED by
//! them — [`reported_segments`] is where the two meet.

use std::time::SystemTime;

use super::{epoch_secs, AccountRelationships, RelationshipPeriod};

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
    /// accounts". `None` for a record that states none, in which case the
    /// organization the account belonged to over the period is read from the
    /// service that owns that fact (see `super::account_relationships`).
    pub management_account_id: Option<String>,
    /// The billing group's primary account, reported "for billing group
    /// members". `None` for a record that states none, in which case the
    /// membership that held over the period — and the domain it shapes the
    /// account's data into — is read from the service that owns that fact (see
    /// `super::account_relationships`).
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

    /// This record as the entry a read reports over `[begin_secs, end_secs)`, a
    /// period of this record over which the account relationships `holds` did
    /// not change.
    ///
    /// What the record states, it states: a relationship member it carries is
    /// what the entry reports for it. A member it leaves absent is not "no
    /// relationship" but a fact this service does not own, so it comes from
    /// `holds` — read, for the period reported, off the store of the service
    /// that owns it.
    ///
    /// The Billing Conductor relationship is ONE fact with the shaping of the
    /// data it causes: `PRO_FORMA` is "data shaped by Billing Conductor" and
    /// `billingGroupPrimaryAccountId` is reported "for billing group members".
    /// So a record stating the billing group it was in states its own domain
    /// too, and one stating no group takes both from the membership that held
    /// over the period — `PRO_FORMA` while the account was a member of a billing
    /// group, and the domain the record was recorded under while it was not.
    fn reported(
        &self,
        key: &str,
        begin_secs: f64,
        end_secs: f64,
        holds: &AccountRelationships,
    ) -> ReportedSegment {
        let (domain, billing_group_primary_account_id) =
            if self.billing_group_primary_account_id.is_some() {
                (
                    self.domain.clone(),
                    self.billing_group_primary_account_id.clone(),
                )
            } else if holds.in_billing_group {
                (
                    BILLING_DOMAIN_PRO_FORMA.to_string(),
                    holds.billing_group_primary_account_id.clone(),
                )
            } else {
                (self.domain.clone(), None)
            };
        ReportedSegment {
            key: entry_key(key, begin_secs),
            domain,
            begin_secs,
            end_secs,
            billing_transfer_account_id: self.billing_transfer_account_id.clone(),
            management_account_id: self
                .management_account_id
                .clone()
                .or_else(|| holds.management_account_id.clone()),
            billing_group_primary_account_id,
        }
    }
}

/// The key one reported entry is walked under: the store key of the record it
/// comes from, then the instant the entry begins, left-padded the way
/// [`BillingViewSegment::key`] pads one.
///
/// A record can report as several entries, so a cursor naming only the record
/// would resume a page before entries the previous one already returned. Keys
/// built this way rise strictly through the walk — record key first, then the
/// entry's own begin — which is the order the pages are cut in.
fn entry_key(record_key: &str, begin_secs: f64) -> String {
    let began = begin_secs.max(0.0) as u64;
    format!("{record_key}\u{1f}{began:0>20}")
}

/// Seconds since the epoch as the modeled `Timestamp` the wire carries — the
/// bound of a clipped segment period, on [`epoch_secs`]' scale.
pub fn timestamp_at_secs(secs: f64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs_f64(secs)
}

/// One segment as a read reports it: the cut point it is walked under, the
/// period it covers inside the requested one, and what held over that period.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportedSegment {
    /// The key this entry is walked under — the store key of the record it
    /// comes from, then the instant this entry begins, so one page resumes
    /// strictly after the last entry the previous page returned whichever
    /// record that entry came from.
    pub key: String,
    /// The `BillingDomain` that applied over the period.
    pub domain: String,
    /// The period itself, in seconds since the epoch, clipped to the requested
    /// one.
    pub begin_secs: f64,
    pub end_secs: f64,
    /// The account relationships that held over it.
    pub billing_transfer_account_id: Option<String>,
    pub management_account_id: Option<String>,
    pub billing_group_primary_account_id: Option<String>,
}

impl ReportedSegment {
    /// Whether this entry and `next` report the same thing over adjoining
    /// periods, so the two are ONE period of unchanged domain and relationships
    /// and belong in one segment.
    fn joins(&self, next: &Self) -> bool {
        self.end_secs == next.begin_secs
            && self.domain == next.domain
            && self.billing_transfer_account_id == next.billing_transfer_account_id
            && self.management_account_id == next.management_account_id
            && self.billing_group_primary_account_id == next.billing_group_primary_account_id
    }
}

/// The segments `records` — one view's recorded timeline, oldest first, each
/// beside its store key — report over the half-open requested period
/// `[begin_secs, end_secs)`, refined by the account `relationships` timeline.
///
/// A recorded segment states the period it covers, the domain it was recorded
/// under and the relationships it states itself; the relationships it states
/// NONE of are the facts of the services that own them, so the reported entry
/// takes those from the period of `relationships` it falls in. A fact that
/// changed inside one record's period therefore reports as several segments,
/// each with its own time range — and a boundary nothing changed at reports as
/// one, because adjoining entries reporting the same values are one period of
/// unchanged relationships ([`ReportedSegment::joins`]).
///
/// Hidden records are left out ("the response omits hidden segments"), and every
/// reported period is clipped to the requested one
/// ([`BillingViewSegment::clipped`]).
pub fn reported_segments<'records>(
    records: impl Iterator<Item = (&'records String, &'records BillingViewSegment)>,
    relationships: &[RelationshipPeriod],
    begin_secs: f64,
    end_secs: f64,
) -> Vec<ReportedSegment> {
    // An engine that holds none of the stores the relationships come from, and
    // an account none of them records anything about, both read as one unchanged
    // period: a standalone account naming no other account.
    let standalone = [RelationshipPeriod::default()];
    let relationships = if relationships.is_empty() {
        &standalone[..]
    } else {
        relationships
    };
    let mut reported: Vec<ReportedSegment> = Vec::new();
    for (key, record) in records {
        if record.hidden || !record.overlaps(begin_secs, end_secs) {
            continue;
        }
        let (record_begin, record_end) = record.clipped(begin_secs, end_secs);
        // Only the entries of THIS record join each other: two records are two
        // recorded periods of the view's own timeline, which the reader reports
        // as they were recorded.
        let mut entries: Vec<ReportedSegment> = Vec::new();
        for (piece_begin, piece_end, holds) in pieces(record_begin, record_end, relationships) {
            let entry = record.reported(key, piece_begin, piece_end, holds);
            if let Some(last) = entries.last_mut() {
                if last.joins(&entry) {
                    last.end_secs = entry.end_secs;
                    continue;
                }
            }
            entries.push(entry);
        }
        reported.extend(entries);
    }
    reported
}

/// The half-open period `[begin_secs, end_secs)` cut at every relationship
/// change inside it: each piece with the relationships that held over it.
fn pieces(
    begin_secs: f64,
    end_secs: f64,
    relationships: &[RelationshipPeriod],
) -> Vec<(f64, f64, &AccountRelationships)> {
    let mut pieces = Vec::new();
    for (index, period) in relationships.iter().enumerate() {
        // A timeline's first period is in effect before every instant it
        // records a change at, and its last one is still in effect.
        let period_begin = period.begin_secs.unwrap_or(f64::NEG_INFINITY);
        let period_end = relationships
            .get(index + 1)
            .and_then(|next| next.begin_secs)
            .unwrap_or(f64::INFINITY);
        let piece_begin = period_begin.max(begin_secs);
        let piece_end = period_end.min(end_secs);
        if piece_begin < piece_end {
            pieces.push((piece_begin, piece_end, &period.relationships));
        }
    }
    pieces
}
