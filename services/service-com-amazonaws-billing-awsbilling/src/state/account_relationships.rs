// SPDX-License-Identifier: CC0-1.0

//! The account relationships a billing-view segment reports, and the instants
//! they changed at.
//!
//! A segment is "a time range during which the billing domain and account
//! relationships for a billing view remained unchanged", and NONE of those
//! relationships is this service's own fact:
//!
//! * whether the account belongs to an organization — and which account manages
//!   it, the modeled `managementAccountId` "included for organization member
//!   accounts" — is AWS Organizations' fact, held in that service's store
//!   together with the instant the account joined;
//! * whether its data is shaped by Billing Conductor — the modeled `PRO_FORMA`
//!   domain, "data shaped by Billing Conductor that doesn't reflect the final
//!   charges owed to Amazon Web Services" — and which account is the primary
//!   account of the billing group it belongs to (`billingGroupPrimaryAccountId`,
//!   "included for billing group members") is Billing Conductor's fact, held
//!   there per BILLING PERIOD as the membership history of each account.
//!
//! So neither is copied into this service's state: both are read, at the instant
//! a read is answered, off the store of the service that OWNS them — the same
//! way a view's resource policy is read off resource sharing's
//! ([`super::attached_resource_policy`]) and the organization gate reads the
//! organization ([`super::organization_management_account_id`]). A relationship
//! cannot then go stale against the resource that produces it, and a change
//! recorded over there is what makes one recorded period answer with several
//! segments.
//!
//! What this module answers is a TIMELINE: the periods over which one account's
//! relationships did not change, oldest first, each carrying what held over it.
//! `super::billing_view_segments` refines a view's recorded timeline with it.
//!
//! The modeled `billingTransferAccountId` is reported "only when the caller is a
//! billing transfer source account", and no service of this emulator records
//! which account billing responsibility is transferred to — Billing Conductor's
//! transfer records name the organization that owns a transfer and the direction
//! it travels in, not an account relationship of the caller — so that member is
//! reported from a recorded segment that states one and from nothing else.

use service_com_amazonaws_billingconductor_awsbillingconductor::state as billing_conductor;
use service_com_amazonaws_organizations_awsorganizationsv20161128::state as organizations_state;

use super::{date_time_secs, month_period};

/// The account relationships that held over one period of a timeline.
///
/// The default — every member absent, no billing group — is what a standalone
/// account reports: one that belongs to no organization and to no billing group,
/// so its data is its own final charges and it names no other account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountRelationships {
    /// The management account of the organization the account belonged to, or
    /// `None` while it belonged to none.
    pub management_account_id: Option<String>,
    /// Whether the account was a member of a billing group over the period —
    /// which is what makes its data Billing Conductor-shaped, whether or not
    /// that group names a primary account.
    pub in_billing_group: bool,
    /// The primary account of that billing group, absent for a group that has
    /// designated none ("the account ID that serves as the main account in a
    /// billing group" is optional).
    pub billing_group_primary_account_id: Option<String>,
}

/// One period of a relationship timeline: when it began, and what held over it.
///
/// The period runs to the beginning of the next entry of the timeline, and the
/// last one is still in effect.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RelationshipPeriod {
    /// When the period began, inclusive, in seconds since the epoch (the scale
    /// [`super::epoch_secs`] puts every stored instant on). `None` for the
    /// period before every recorded change — the one in effect before any
    /// instant a caller can ask about.
    pub begin_secs: Option<f64>,
    /// The relationships that held over it.
    pub relationships: AccountRelationships,
}

/// When `account_id` joined an organization, and which account manages it.
#[derive(Debug, Clone)]
struct OrganizationMembership {
    joined_secs: f64,
    management_account_id: String,
}

/// One recorded billing-group membership of the account: the half-open period it
/// was in force over, in seconds since the epoch, and the primary account of the
/// group it is a membership of.
#[derive(Debug, Clone)]
struct BillingGroupMembership {
    begin_secs: f64,
    end_secs: Option<f64>,
    primary_account_id: Option<String>,
}

/// The relationship timeline of `account_id`: the periods over which its
/// relationships did not change, oldest first, each with what held over it.
///
/// Never empty — an account whose relationships have never changed has ONE
/// period, bounded on neither side, which is the whole of its history. Every
/// other entry begins at an instant one of the owning services records a change
/// at: the instant the account joined an organization, and the first instant of
/// the billing period a billing-group membership began or ended in.
///
/// `engine` is the handler's own `&*self.engine`. Each store is read under its
/// own guard, and both are released before this answers, so a caller holds none
/// of them while it locks its own.
pub fn account_relationship_timeline<Eng: Send + Sync + 'static>(
    engine: &Eng,
    account_id: &str,
) -> Vec<RelationshipPeriod> {
    let organization = organization_membership(engine, account_id);
    let memberships = billing_group_memberships(engine, account_id);

    // Every instant one of those facts changed at, oldest first. The period
    // before all of them is the unbounded one every timeline opens with.
    let mut changes: Vec<f64> = Vec::new();
    if let Some(organization) = &organization {
        changes.push(organization.joined_secs);
    }
    for membership in &memberships {
        changes.push(membership.begin_secs);
        changes.extend(membership.end_secs);
    }
    changes.sort_by(|left, right| left.partial_cmp(right).unwrap_or(::std::cmp::Ordering::Equal));
    changes.dedup();

    let mut timeline: Vec<RelationshipPeriod> = Vec::with_capacity(changes.len() + 1);
    for begin_secs in ::std::iter::once(None).chain(changes.into_iter().map(Some)) {
        let relationships = relationships_at(begin_secs, organization.as_ref(), &memberships);
        // An instant at which nothing the timeline reports actually changed is
        // no change at all — a membership that covered no billing period, or two
        // facts that moved together — so it opens no second period: the periods
        // this answers are the ones over which the relationships "remained
        // unchanged".
        if timeline
            .last()
            .is_some_and(|last| last.relationships == relationships)
        {
            continue;
        }
        timeline.push(RelationshipPeriod {
            begin_secs,
            relationships,
        });
    }
    timeline
}

/// The relationships in effect at `begin_secs`, out of the facts the owning
/// services recorded. `None` is the instant before every recorded change.
fn relationships_at(
    begin_secs: Option<f64>,
    organization: Option<&OrganizationMembership>,
    memberships: &[BillingGroupMembership],
) -> AccountRelationships {
    let at = begin_secs.unwrap_or(f64::NEG_INFINITY);
    let membership = memberships
        .iter()
        // The NEWEST membership covering the instant, the way Billing Conductor
        // itself decides which membership a billing period reports: an account
        // released and re-associated inside one period is in the group it ended
        // up in.
        .rev()
        .find(|membership| {
            membership.begin_secs <= at && membership.end_secs.is_none_or(|end| at < end)
        });
    AccountRelationships {
        management_account_id: organization
            .filter(|organization| at >= organization.joined_secs)
            .map(|organization| organization.management_account_id.clone()),
        in_billing_group: membership.is_some(),
        billing_group_primary_account_id: membership
            .and_then(|membership| membership.primary_account_id.clone()),
    }
}

/// The organization membership AWS Organizations records for `account_id`, or
/// `None` while that store holds no organization, holds no record of this
/// account being a member of it, or is not mounted on this engine at all — the
/// three ways an account belongs to no organization.
fn organization_membership<Eng: Send + Sync + 'static>(
    engine: &Eng,
    account_id: &str,
) -> Option<OrganizationMembership> {
    let organizations = organizations_state::service_state(engine)?;
    let organizations = organizations.lock();
    let organization = organizations.organization.as_ref()?;
    let account = organizations.accounts.get(account_id)?;
    Some(OrganizationMembership {
        // When the account joined is that store's own record of it — what its
        // `JoinedTimestamp` reports — so the segment boundary is the instant the
        // relationship began, not the instant this read noticed it.
        joined_secs: date_time_secs(&account.joined_timestamp),
        management_account_id: organization.master_account_id.clone(),
    })
}

/// The billing-group memberships Billing Conductor records for `account_id`,
/// oldest first, as instants.
///
/// A membership is held over BILLING PERIODS — "the first billing period the
/// account is a member of that group for", up to the first period it is no
/// longer one — so its bounds are the first instant of those UTC calendar
/// months, read off the civil calendar the same way every other month of this
/// service is ([`month_period`]). An empty answer is an account that store has
/// no membership for, and one read on an engine where Billing Conductor is not
/// mounted at all.
fn billing_group_memberships<Eng: Send + Sync + 'static>(
    engine: &Eng,
    account_id: &str,
) -> Vec<BillingGroupMembership> {
    let Some(billing_conductor) = billing_conductor::service_state(engine) else {
        return Vec::new();
    };
    let billing_conductor = billing_conductor.lock();
    let Some(memberships) = billing_conductor.account_associations.get(account_id) else {
        return Vec::new();
    };
    memberships
        .iter()
        .filter_map(|membership| {
            let (begin, _) = month_period(&membership.inclusive_start_billing_period)?;
            let end = match &membership.exclusive_end_billing_period {
                Some(month) => Some(month_period(month)?.0),
                None => None,
            };
            Some(BillingGroupMembership {
                begin_secs: date_time_secs(&begin),
                end_secs: end.as_ref().map(date_time_secs),
                // The group's primary account is the group's own record, so a
                // membership of a group this store no longer holds names none:
                // nothing is reported that the owning service cannot answer for.
                primary_account_id: billing_conductor
                    .billing_groups
                    .get(&membership.billing_group_arn)
                    .and_then(|group| group.primary_account_id.clone()),
            })
        })
        .collect()
}
