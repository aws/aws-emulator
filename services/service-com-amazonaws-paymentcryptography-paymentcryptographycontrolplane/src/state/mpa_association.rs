// SPDX-License-Identifier: CC0-1.0

//! The Multi-Party Approval (MPA) team association a protected operation of this service
//! carries: the record behind `AssociateMpaTeam`, `GetMpaTeamAssociation` and
//! `DisassociateMpaTeam`.
//!
//! One association per protected operation. `MpaOperation` — the modelled `Action` those
//! three operations take — is the identity of the record, so the store holds at most one
//! association per value that enumeration declares
//! (`smithy_query(query="shape",shape="MpaOperation")`) and no bound of this emulator's
//! own is needed for it.
//!
//! `AssociationState` names the two states where the association is still settling
//! (`UPDATE_PENDING` while the association a caller asked for is taking effect,
//! `DELETE_PENDING` while one is being torn down) and the one where it has settled
//! (`ACTIVE`) — `smithy_query(query="shape",shape="AssociationState")`. The move between
//! them is a transition the CLOCK owes: it falls due [`MPA_ASSOCIATION_SPAN`] after the
//! call that asked for it and is derived from the stored instant, never slept for. The
//! span is deliberately non-zero, so a caller that reads the association straight after
//! the call still observes the state the association is settling through.
//!
//! The approval team an association names is NOT this service's resource: an
//! `MpaTeamArn` names an approval team AWS Multi-Party Approval mints, so whether one
//! exists is answered by that service's own store through the shared engine
//! ([`approval_team_exists`]) rather than by reading the ARN's text or by keeping a
//! second registry here.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::MpaTeamAssociation;

/// How long an MPA team association takes to settle, in either direction.
///
/// A span of this emulator's choosing: AWS publishes no duration for it, and the two
/// non-terminal states the model declares (`UPDATE_PENDING`, `DELETE_PENDING`) exist
/// because neither move is instantaneous. Nothing in this operation's documentation makes
/// the association itself wait on an approval — `AssociateMpaTeam` "Associates a
/// Multi-Party Approval (MPA) team with a protected operation", and it is the protected
/// OPERATION that the team then approves — so what settles here is the association taking
/// effect and not an approval outcome, which is also why no MPA session is named for it
/// ([`StoredMpaAssociation::to_association`]).
pub const MPA_ASSOCIATION_SPAN: Duration = Duration::from_secs(3);

/// `AssociationState` values, from
/// `smithy_query(query="shape",shape="AssociationState")`.
pub const ASSOCIATION_STATE_ACTIVE: &str = "ACTIVE";
/// See [`ASSOCIATION_STATE_ACTIVE`].
pub const ASSOCIATION_STATE_UPDATE_PENDING: &str = "UPDATE_PENDING";
/// See [`ASSOCIATION_STATE_ACTIVE`].
pub const ASSOCIATION_STATE_DELETE_PENDING: &str = "DELETE_PENDING";

/// One protected operation's MPA team association, as this store holds it.
#[derive(Debug, Clone)]
pub struct StoredMpaAssociation {
    /// The protected operation this association is for: the modelled `Action`, which is
    /// also this record's key in the store.
    pub action: String,
    /// The ARN of the approval team the association names.
    pub mpa_team_arn: String,
    /// When the call that asked for this association ran — the instant the move to
    /// `ACTIVE` is due [`MPA_ASSOCIATION_SPAN`] after.
    pub requested: SystemTime,
    /// When the call that asked for this association to be torn down ran, if one has:
    /// the instant the record's removal is due [`MPA_ASSOCIATION_SPAN`] after.
    pub teardown_requested: Option<SystemTime>,
}

impl StoredMpaAssociation {
    /// The association `action` names a team for, as `AssociateMpaTeam` records it.
    pub fn requested(action: &str, mpa_team_arn: &str, now: SystemTime) -> Self {
        Self {
            action: action.to_string(),
            mpa_team_arn: mpa_team_arn.to_string(),
            requested: now,
            teardown_requested: None,
        }
    }

    /// The `AssociationState` this association reports at `now`: `DELETE_PENDING` once a
    /// teardown was asked for, `UPDATE_PENDING` until the association's own span has
    /// elapsed, and `ACTIVE` after that.
    pub fn effective_state(&self, now: SystemTime) -> &'static str {
        if self.teardown_requested.is_some() {
            return ASSOCIATION_STATE_DELETE_PENDING;
        }
        if elapsed_since(self.requested, now) {
            ASSOCIATION_STATE_ACTIVE
        } else {
            ASSOCIATION_STATE_UPDATE_PENDING
        }
    }

    /// Whether the teardown this record was asked for is due at `now`, so the record is
    /// one the clock has already removed.
    pub fn is_torn_down(&self, now: SystemTime) -> bool {
        self.teardown_requested
            .is_some_and(|requested| elapsed_since(requested, now))
    }

    /// This association as the modelled `MpaTeamAssociation` structure, in the state it
    /// reports at `now`.
    ///
    /// `MpaStatus` is left unset: it reports the MPA SESSION an association is waiting on
    /// ("The MPA session status for the association, if applicable"), and this emulator
    /// starts no session in AWS Multi-Party Approval, so there is no session to name and
    /// none is invented — the member is optional precisely because an association need not
    /// be waiting on one.
    pub fn to_association(&self, now: SystemTime) -> MpaTeamAssociation {
        MpaTeamAssociation {
            action: self.action.clone(),
            mpa_team_arn: self.mpa_team_arn.clone(),
            association_state: self.effective_state(now).to_string(),
            mpa_status: None,
        }
    }
}

/// Whether [`MPA_ASSOCIATION_SPAN`] has passed between `start` and `now`.
///
/// A clock that went backwards between the two reads leaves the span unelapsed rather
/// than reporting a settled transition, so the state a caller reads never moves back.
fn elapsed_since(start: SystemTime, now: SystemTime) -> bool {
    now.duration_since(start)
        .is_ok_and(|elapsed| elapsed >= MPA_ASSOCIATION_SPAN)
}

/// Drop every association whose teardown the clock has already completed.
///
/// Materialises a change the CLOCK owed, not one this call asks for: the record was due
/// to be gone at the same instant whether or not anybody read it, so a reporting
/// operation applies it too.
pub fn settle(associations: &mut BTreeMap<String, StoredMpaAssociation>, now: SystemTime) {
    associations.retain(|_, association| !association.is_torn_down(now));
}

/// Whether AWS Multi-Party Approval holds an approval team under `mpa_team_arn`: the ONE
/// read of that registry by this crate.
///
/// MPA owns the approval team — its `CreateApprovalTeam` is the only call that mints one
/// and its deletion handshake the only one that drops it — so an association names a team
/// that EXISTS, or it names nothing, and this service resolves that question in MPA's own
/// store rather than in a registry of its own.
///
/// `None` when that store cannot be reached at all (the engine is not the shared
/// in-memory one), which a caller answers with a modelled server error exactly as it
/// answers its own store being unavailable — never by treating the team as absent.
///
/// LOCK ORDER: this takes MPA's store lock, holds no other while it does, and hands back
/// an owned value rather than a guard — so a caller asks it with NO guard of this
/// service's store held, which keeps the cross-service read one-directional and a lock
/// cycle between the two stores unreachable.
pub fn approval_team_exists<Eng: Send + Sync + 'static>(
    engine: &Eng,
    mpa_team_arn: &str,
) -> Option<bool> {
    let state = ::service_com_amazonaws_mpa_awsfluffycoreservice::state::service_state(engine)?;
    let holds = state.lock().team(mpa_team_arn).is_some();
    Some(holds)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The action every row below associates a team for: the one value `MpaOperation`
    /// declares.
    const ACTION: &str = "IMPORT_ROOT_PUBLIC_KEY_CERTIFICATE";

    /// An approval-team ARN of the form `MpaTeamArn`'s modelled pattern admits.
    fn team_arn(name: &str) -> String {
        format!("arn:aws:mpa:us-west-2:000000000000:approval-team/{name}")
    }

    /// RULE: the state an association reports is derived from the stored instants and the
    /// clock — pending until its span has elapsed, active after, and delete-pending from
    /// the moment a teardown is asked for.
    #[test]
    fn the_reported_state_is_the_one_the_clock_makes_due() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let association = StoredMpaAssociation::requested(ACTION, &team_arn("alpha"), start);
        // (case, how long after the request, the state expected)
        let rows: &[(&str, Duration, &str)] = &[
            ("at the instant of the call", Duration::ZERO, ASSOCIATION_STATE_UPDATE_PENDING),
            (
                "just before the span elapses",
                MPA_ASSOCIATION_SPAN - Duration::from_millis(1),
                ASSOCIATION_STATE_UPDATE_PENDING,
            ),
            ("as the span elapses", MPA_ASSOCIATION_SPAN, ASSOCIATION_STATE_ACTIVE),
            (
                "long after the span elapses",
                MPA_ASSOCIATION_SPAN * 1000,
                ASSOCIATION_STATE_ACTIVE,
            ),
        ];
        for (case, after, expected) in rows {
            let reported = association.effective_state(start + *after);
            assert_eq!(
                reported, *expected,
                "case {case:?}: expected {expected}, got {reported}"
            );
        }

        let mut torn_down = association.clone();
        torn_down.teardown_requested = Some(start + MPA_ASSOCIATION_SPAN * 10);
        let teardown_at = start + MPA_ASSOCIATION_SPAN * 10;
        for (case, at) in [
            ("at the teardown call", teardown_at),
            ("while the teardown settles", teardown_at + Duration::from_millis(1)),
        ] {
            let reported = torn_down.effective_state(at);
            assert_eq!(
                reported, ASSOCIATION_STATE_DELETE_PENDING,
                "case {case:?}: expected {ASSOCIATION_STATE_DELETE_PENDING}, got {reported}"
            );
            assert!(
                !torn_down.is_torn_down(at),
                "case {case:?}: the record must still be held while the teardown settles"
            );
        }
        assert!(
            torn_down.is_torn_down(teardown_at + MPA_ASSOCIATION_SPAN),
            "the record must be gone once the teardown span has elapsed"
        );
    }

    /// RULE: `settle` drops exactly the associations whose teardown the clock completed.
    #[test]
    fn settling_drops_only_the_associations_the_clock_removed() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut associations = BTreeMap::new();
        let mut settling = StoredMpaAssociation::requested(ACTION, &team_arn("alpha"), start);
        settling.teardown_requested = Some(start);
        associations.insert(ACTION.to_string(), settling);
        associations.insert(
            "OTHER_PROTECTED_OPERATION".to_string(),
            StoredMpaAssociation::requested("OTHER_PROTECTED_OPERATION", &team_arn("beta"), start),
        );

        settle(&mut associations, start + Duration::from_millis(1));
        assert_eq!(
            associations.len(),
            2,
            "case still settling: nothing is dropped before the span elapses, got {associations:?}"
        );

        settle(&mut associations, start + MPA_ASSOCIATION_SPAN);
        let held: Vec<&String> = associations.keys().collect();
        assert_eq!(
            held,
            vec!["OTHER_PROTECTED_OPERATION"],
            "case settled: only the torn-down association is dropped, got {held:?}"
        );
    }
}
