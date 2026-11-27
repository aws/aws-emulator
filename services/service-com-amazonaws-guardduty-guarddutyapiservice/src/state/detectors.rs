// SPDX-License-Identifier: CC0-1.0

//! The detector and the accounts around it: the record every other resource of
//! this service hangs off, plus the member accounts, the administrator and the
//! invitations that bind two accounts together.
//!
//! One file for the detector's own record and the reads that project it, so a
//! detector's collections and the member relationships over them are held in one
//! place.

use super::*;

/// A GuardDuty detector: the account/Region-level resource the whole service hangs
/// off.
#[derive(Debug, Clone)]
pub struct Detector {
    pub id: String,
    /// The Region whose endpoint created this detector, and the only one that
    /// serves it.
    pub region: String,
    /// `DetectorStatus`: [`ENABLED`] or [`DISABLED`].
    pub status: String,
    pub service_role: String,
    /// The Malware Protection service-linked role, which is a DIFFERENT role from
    /// the detector-level [`Self::service_role`]:
    /// `MalwareProtectionConfigurationResult.ServiceRole` is "The GuardDuty Malware
    /// Protection service role"
    /// (`smithy_query(shape="MalwareProtectionConfigurationResult")`).
    pub malware_protection_service_role: String,
    pub finding_publishing_frequency: Option<String>,
    pub features: Vec<FeatureRecord>,
    pub tags: BTreeMap<String, String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    /// This detector's findings, keyed by finding id.
    pub findings: BTreeMap<String, FindingRecord>,
    /// This detector's filters, keyed by filter name. `DeleteDetector` drops them
    /// with the detector.
    pub filters: BTreeMap<String, FilterRecord>,
    /// This detector's publishing destinations, keyed by `DestinationId`. A
    /// destination belongs to the detector it was created under, so
    /// `DeleteDetector` drops them with the detector.
    ///
    /// BOUNDED by [`PUBLISHING_DESTINATIONS_PER_DETECTOR`]: a destination is
    /// retained until its detector is deleted and has no expiry of its own, and
    /// each one carries the `ClientToken` that minted it
    /// ([`Detector::publishing_destination_for_token`]), so the idempotency index
    /// is bounded by the same maximum rather than growing with the
    /// REQUEST-chosen tokens.
    pub publishing_destinations: BTreeMap<String, PublishingDestinationRecord>,
    /// This detector's ThreatIntelSets, keyed by minted `ThreatIntelSetId`.
    /// `DeleteDetector` drops them with the detector.
    pub threat_intel_sets: BTreeMap<String, ThreatIntelSetRecord>,
    /// This detector's investigations, keyed by investigation id. They hang off the
    /// detector like its findings and filters, so `DeleteDetector` drops them with
    /// it.
    pub investigations: BTreeMap<String, InvestigationRecord>,
    /// This detector's trusted entity sets, keyed by the minted entity-set id.
    /// They hang off the detector — every operation on one is reached through
    /// `/detector/{DetectorId}/trustedentityset`
    /// (`smithy_query(operation="CreateTrustedEntitySet")`) — so
    /// `DeleteDetector` drops them with it.
    pub trusted_entity_sets: BTreeMap<String, EntitySetRecord>,
    /// This detector's threat entity sets, keyed by the minted entity-set id.
    /// They hang off the detector like the trusted entity sets above — every
    /// operation on one is reached through
    /// `/detector/{DetectorId}/threatentityset`
    /// (`smithy_query(operation="CreateThreatEntitySet")`) — so `DeleteDetector`
    /// drops them with it.
    ///
    /// BOUNDED by [`THREAT_ENTITY_SETS_PER_DETECTOR`], which
    /// `CreateThreatEntitySet` enforces before it writes.
    pub threat_entity_sets: BTreeMap<String, EntitySetRecord>,
    /// This detector's IPSets (trusted IP lists), keyed by the minted `IpSetId`.
    /// They are sub-resources of the detector — every IPSet operation is bound
    /// under `/detector/{DetectorId}/ipset`
    /// (`smithy_query(operation="GetIPSet")`) — so `DeleteDetector` drops them
    /// with it.
    pub ip_sets: BTreeMap<String, IpSetRecord>,
    /// This detector's Malware Protection scan settings.
    pub malware_scan_settings: MalwareScanSettings,
    /// The member accounts this detector's account administers, keyed by account
    /// id. `DeleteDetector` drops them with the detector, like the findings and
    /// filters above.
    pub members: BTreeMap<String, MemberRecord>,
    /// The invitations this detector's ACCOUNT has received, keyed by the account
    /// id of the administrator that sent each one.
    ///
    /// The inbox side of the member relationship: `InviteMembers` is called by an
    /// administrator and sends an invitation the invited account then answers, so
    /// an entry here is an invitation that was SENT and is still pending — which
    /// is what `AcceptInvitation` / `AcceptAdministratorInvitation` resolve
    /// `(MasterId, InvitationId)` against, and what `ListInvitations` reports.
    ///
    /// WRITTEN by `InviteMembers`, which is the one operation that SENDS an
    /// invitation: sending one is a write to both sides of the relationship — the
    /// administrator's own view of the account it invited ([`MemberRecord`]) and
    /// the pending invitation the invited account answers — so the send delivers
    /// it here through
    /// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::deliver_invitation_in`].
    /// Every account this emulator presents carries the one canonical account id
    /// ([`administrator_account_id`]), the sending administrator included, so the
    /// inbox an invitation is delivered to is the one the Region's endpoint
    /// serves and the key it lands under is that account id.
    ///
    /// This is state rather than a constant because both the refusal and the
    /// report are a lookup: an invitation this store holds is accepted by one and
    /// reported by the other, read through [`Self::received_invitations_in`] and
    /// projected through [`invitation_view`]. `GetInvitationsCount` reports the
    /// SIZE of that same inbox, through the same reader. `DeleteInvitations` and
    /// `DeclineInvitations` both TAKE one out, through
    /// [`Self::take_received_invitations_in`], so an invitation this store holds
    /// leaves the inbox accepted, declined or deleted and never twice.
    ///
    /// BOUNDED by [`MAX_INVITATIONS_PER_DETECTOR`], so the delivery cannot grow
    /// it without limit: the key is an account id, which is request-chosen.
    pub received_invitations: BTreeMap<String, ReceivedInvitationRecord>,
    /// The administrator this detector's account is monitored by — what
    /// `GetMasterAccount` / `GetAdministratorAccount` report — or `None` while the
    /// account has accepted no invitation.
    ///
    /// Written only by accepting an invitation, and dropped by
    /// `DisassociateFromMasterAccount`; a detector a create just minted
    /// administers itself, so it starts with none.
    pub administrator: Option<AdministratorRecord>,
    /// What this detector's account has configured for the OTHER accounts in its
    /// organization — what `DescribeOrganizationConfiguration` reports and
    /// `UpdateOrganizationConfiguration` writes.
    ///
    /// It hangs off the detector because that is what identifies it on the wire
    /// (`/detector/{DetectorId}/admin`), so `DeleteDetector` drops it with the
    /// detector. Bounded by construction: one record per `AutoEnableMembers` value
    /// and one per [`ORG_FEATURES`] member, whatever a caller sends.
    pub organization: OrganizationConfiguration,
}

/// An invitation this detector's account RECEIVED, as the store holds it.
///
/// Every member is one the `Invitation` shape carries
/// (`smithy_query(shape="Invitation")`), with the instant held as epoch
/// milliseconds — the single source for the `InvitedAt` anything reports.
#[derive(Debug, Clone)]
pub struct ReceivedInvitationRecord {
    /// `Invitation.AccountId` — "the ID of the account that the invitation was
    /// sent from", which is the `MasterId` / `AdministratorId` an acceptance names.
    pub administrator_account_id: String,
    /// `Invitation.InvitationId` — "the ID of the invitation. This value is used
    /// to validate the inviter account to the member account".
    pub invitation_id: String,
    /// `Invitation.RelationshipStatus`: [`RELATIONSHIP_INVITED`] while the
    /// invitation is pending.
    pub relationship_status: String,
    /// When the invitation was sent.
    pub invited_at_millis: i64,
}

/// The administrator relationship an accepted invitation established.
///
/// Every member is one the `Master` shape carries
/// (`smithy_query(shape="Master")`); `GetAdministratorAccount` reports the same
/// facts under `Administrator`.
#[derive(Debug, Clone)]
pub struct AdministratorRecord {
    /// `Master.AccountId` — "the ID of the account used as the administrator
    /// account".
    pub account_id: String,
    /// `Master.InvitationId` — "the value used to validate the administrator
    /// account to the member account": the id of the invitation that was accepted.
    pub invitation_id: String,
    /// `Master.RelationshipStatus`: [`RELATIONSHIP_ENABLED`] once the invitation
    /// has been accepted.
    pub relationship_status: String,
    /// When the accepted invitation was sent — `Master.InvitedAt`, which is the
    /// invitation's instant and not the acceptance's.
    pub invited_at_millis: i64,
}

/// A member account of the administrator that owns the detector this record hangs
/// off.
///
/// The record outlives the ASSOCIATION: when a member is disassociated "the member
/// account details obtained from the CreateMembers API, including the associated
/// email addresses, are retained. This is done so that the delegated administrator
/// can invoke the InviteMembers API without the need to invoke the CreateMembers
/// API again. To remove the details associated with a member account, the delegated
/// administrator must invoke the DeleteMembers API"
/// (`smithy_query(query="doc",shape="DisassociateMembers")`). So disassociating
/// moves [`Self::relationship_status`] and nothing else, and only `DeleteMembers`
/// drops the record.
#[derive(Debug, Clone)]
pub struct MemberRecord {
    pub account_id: String,
    /// The member account's email address, as the request that recorded the account
    /// gave it.
    pub email: String,
    /// `Member.RelationshipStatus`: [`RELATIONSHIP_CREATED`],
    /// [`RELATIONSHIP_INVITED`] once `InviteMembers` has sent the account an
    /// invitation — or [`RELATIONSHIP_EMAIL_VERIFICATION_FAILED`] when that send
    /// could not verify the account's address — [`RELATIONSHIP_DISABLED`] once
    /// `StopMonitoringMembers` has stopped monitoring the account,
    /// [`RELATIONSHIP_ENABLED`] once `StartMonitoringMembers` has turned monitoring
    /// back on for it, or [`RELATIONSHIP_REMOVED`] once `DisassociateMembers` has
    /// ended the association.
    pub relationship_status: String,
    /// When the invitation `InviteMembers` sent this account was sent, in epoch
    /// milliseconds — the member's `InvitedAt`, "the timestamp when the invitation
    /// was sent" (`smithy_query(shape="Member",member="InvitedAt")`) — or `None`
    /// while the account has been recorded and not invited, which is why the member
    /// is optional.
    ///
    /// Retained across a disassociation like the rest of the record: the invitation
    /// WAS sent at that instant, and only `DeleteMembers` drops the record that says
    /// so.
    pub invited_at_millis: Option<i64>,
    /// When the relationship last changed, in epoch milliseconds — the member's
    /// `UpdatedAt`.
    ///
    /// The RELATIONSHIP's instant, not the configuration's:
    /// [`Self::features`] carries its own per-feature `UpdatedAt`, so configuring
    /// the member account's detector moves that and leaves this where it was.
    pub updated_at_millis: i64,
    /// The configuration of the member account's own detector, as the
    /// administrator maintains it — what `UpdateMemberDetectors` writes and
    /// `GetMemberDetectors` reports, under both the `Features` and the deprecated
    /// `DataSources` view.
    ///
    /// Held as the same [`FeatureRecord`] list a detector carries, because it is
    /// the same configuration seen from the administrator's side: one stored
    /// switch per feature, each with the instant it last changed. A member starts
    /// with the feature set a detector created without a `Features` list gets
    /// ([`default_features`]), and only the features the `OrgFeature` enum carries
    /// ([`MEMBER_FEATURES`]) can be configured through a member request.
    pub features: Vec<FeatureRecord>,
}

impl MemberRecord {
    /// Is this member currently ASSOCIATED with the administrator?
    ///
    /// "Is a member account of this administrator" and "is associated with it" are
    /// two different questions, because a record is held before the association
    /// begins and retained after it ends (see the type's documentation):
    /// `GetMembers` answers the first, and the operations that act on the
    /// association answer the second.
    ///
    /// The members that are NOT associated are the two classes `ListMembers` names
    /// when it offers "to only return associated members or to return all members
    /// (including members who haven't been invited yet or have been disassociated)"
    /// (`smithy_query(query="doc",shape="ListMembersRequest",member="OnlyAssociated")`):
    /// an account the administrator has recorded and not invited yet
    /// ([`RELATIONSHIP_CREATED`]), and one a disassociation moved
    /// ([`RELATIONSHIP_REMOVED`]). The invitation is what associates the account, so
    /// the relationship is an association from there on — whether or not the account
    /// has answered it, which is the narrower [`Self::is_monitored`], and whether or
    /// not its address could be verified
    /// ([`RELATIONSHIP_EMAIL_VERIFICATION_FAILED`]), since that send delivered the
    /// invitation too.
    pub fn is_associated(&self) -> bool {
        self.relationship_status == RELATIONSHIP_INVITED
            || self.relationship_status == RELATIONSHIP_EMAIL_VERIFICATION_FAILED
            || self.relationship_status == RELATIONSHIP_ENABLED
            || self.relationship_status == RELATIONSHIP_DISABLED
    }

    /// Is GuardDuty MONITORING this member account under the administrator?
    ///
    /// The association and the monitoring are two facts, and the monitoring is the
    /// narrower one: it begins when the member ACCOUNT accepts the administrator's
    /// invitation — [`RELATIONSHIP_ENABLED`] while it runs and
    /// [`RELATIONSHIP_DISABLED`] while the administrator has stopped it, which is the
    /// state `StartMonitoringMembers` turns back on ("use this operation to restart
    /// monitoring of accounts that you stopped monitoring with the
    /// StopMonitoringMembers operation",
    /// `smithy_query(query="doc",shape="StartMonitoringMembers")`). An account the
    /// administrator has recorded or invited is not monitored under it yet, whatever
    /// else it may already be associated with it for.
    pub fn is_monitored(&self) -> bool {
        self.relationship_status == RELATIONSHIP_ENABLED
            || self.relationship_status == RELATIONSHIP_DISABLED
    }

    /// Has the administrator ENDED this member's association with
    /// `DisassociateMembers`, leaving the record it retains behind
    /// ([`RELATIONSHIP_REMOVED`])?
    ///
    /// Not the complement of [`Self::is_associated`]: an account recorded and not
    /// invited yet, and one whose invitation is still unanswered, are neither
    /// associated nor disassociated, which is why the two questions are asked
    /// separately.
    pub fn is_disassociated(&self) -> bool {
        self.relationship_status == RELATIONSHIP_REMOVED
    }

    /// Set one feature's status on the member account's configuration, recording
    /// `now` as its `UpdatedAt` — [`Detector::set_feature`] for the member side of
    /// the same switch.
    pub fn set_feature(&mut self, name: &str, status: &str, now: i64) {
        set_feature_in(&mut self.features, name, status, now);
    }

    /// What one of the member's data sources reports: the status of the feature
    /// that governs it and the instant that feature last changed.
    pub fn data_source(&self, feature: &str) -> (String, i64) {
        feature_data_source(&self.features, feature, self.updated_at_millis)
    }
}

/// The modelled `Member` a read reports for one stored member account.
///
/// `ListMembers` and `GetMembers` both report the `Members` list, so the projection
/// lives here once: two reads of one member must not disagree.
///
/// `MasterId` and `AdministratorId` are both "the administrator account ID"
/// (`smithy_query(shape="Member")`) — the same fact under the old and the current
/// name — so both carry the account that owns the detector this member hangs off.
pub fn member_view(
    member: &MemberRecord,
    administrator: &str,
) -> crate::ops::types::shared_types::Member {
    crate::ops::types::shared_types::Member {
        account_id: member.account_id.clone(),
        // The member account's own detector is not a resource this emulator holds;
        // the member is optional, so it is absent rather than invented.
        detector_id: None,
        master_id: administrator.to_string(),
        administrator_id: Some(administrator.to_string()),
        email: member.email.clone(),
        relationship_status: member.relationship_status.clone(),
        // Present only once an invitation has been sent to the account, which is
        // what `InviteMembers` does; a recorded, un-invited member has no instant at
        // which one was sent, so the optional member is absent rather than invented.
        invited_at: member.invited_at_millis.map(iso8601),
        updated_at: iso8601(member.updated_at_millis),
    }
}

/// The modelled `Invitation` a read reports for one invitation in the inbox.
///
/// Every member of the shape is optional (`smithy_query(shape="Invitation")`) and
/// every one of them is a fact the record holds, so the projection fills them all
/// and lives here once: the operations that report the inbox must not disagree
/// about one invitation.
pub fn invitation_view(
    invitation: &ReceivedInvitationRecord,
) -> crate::ops::types::list_invitations_types::Invitation {
    crate::ops::types::list_invitations_types::Invitation {
        // "The ID of the account that the invitation was sent from"
        // (`smithy_query(shape="Invitation")`), which is the key the inbox holds
        // the record under.
        account_id: Some(invitation.administrator_account_id.clone()),
        invitation_id: Some(invitation.invitation_id.clone()),
        relationship_status: Some(invitation.relationship_status.clone()),
        // `Invitation.InvitedAt` targets a `String`, rendered the way every other
        // instant this service reports as text is.
        invited_at: Some(iso8601(invitation.invited_at_millis)),
    }
}

impl Detector {
    /// The status of one feature, or [`DISABLED`] when the detector does not carry
    /// it.
    pub fn feature_status(&self, name: &str) -> &str {
        feature_status_in(&self.features, name)
    }

    /// What one data source reports: the status of the feature that governs it and
    /// the instant that feature's status last changed, in epoch milliseconds.
    ///
    /// The status is the feature's OWN status, never a function of the detector's
    /// `Status` member: that member carries whether the detector is enabled, while
    /// the feature and data-source views carry the configuration, which a detector
    /// keeps across being disabled and enabled again.
    pub fn data_source(&self, feature: &str) -> (String, i64) {
        feature_data_source(&self.features, feature, self.updated_at_millis)
    }

    /// Materialise every threat list's clock-owed status change, so no two readers
    /// of this detector disagree about where a list has got to.
    pub fn settle_threat_intel_sets(&mut self, now_millis: i64) {
        for set in self.threat_intel_sets.values_mut() {
            set.settle(now_millis);
        }
    }

    /// The threat list `id`, if this detector holds it.
    pub fn threat_intel_set(&self, id: &str) -> Option<&ThreatIntelSetRecord> {
        self.threat_intel_sets.get(id)
    }

    /// The threat list a create carrying `token` already minted, if any — the
    /// `@idempotencyToken` lookup, scoped to the detector the list hangs off.
    ///
    /// A list a delete has already accepted ([`ThreatIntelSetRecord::is_deleting`])
    /// resolves to nothing, so a replayed create never answers with a list the
    /// service is no longer keeping — the same rule [`Self::ip_set_for_token`]
    /// follows for a deleted IPSet.
    pub fn threat_intel_set_for_token(&self, token: &str) -> Option<&ThreatIntelSetRecord> {
        self.threat_intel_sets
            .values()
            .find(|set| set.client_token.as_deref() == Some(token) && !set.is_deleting())
    }

    /// How many of this detector's threat lists are LIVE — held and not being
    /// deleted.
    ///
    /// This is the count [`THREAT_INTEL_SETS_PER_DETECTOR`] bounds: a list whose
    /// delete has been accepted is only still REPORTED, never used, so it occupies
    /// none of the detector's slots — the same rule
    /// [`Self::live_ip_set_count`] applies to a deleted IPSet.
    pub fn live_threat_intel_set_count(&self) -> usize {
        self.threat_intel_sets
            .values()
            .filter(|set| !set.is_deleting())
            .count()
    }

    /// The ids of the LIVE threat lists this detector holds, in
    /// `ThreatIntelSetId` order (the id is the key they are stored under, so the
    /// order is TOTAL — what makes a page token mean the same thing on the call
    /// that handed it back).
    ///
    /// LIVE is the same set [`Self::live_threat_intel_set_count`] counts: a list
    /// whose delete has been accepted is no longer one of the detector's threat
    /// lists, so `ListThreatIntelSets` stops naming it. Its record is still
    /// retained ([`DELETED_THREAT_INTEL_SETS_RETAINED`]) so `GetThreatIntelSet` can
    /// report the status the delete moved it to — being readable on its own id is
    /// not the same as being part of the collection a list enumerates, which is the
    /// rule [`Self::live_ip_set_ids`] follows for a deleted IPSet.
    pub fn live_threat_intel_set_ids(&self) -> Vec<String> {
        self.threat_intel_sets
            .values()
            .filter(|set| !set.is_deleting())
            .map(|set| set.id.clone())
            .collect()
    }

    /// Accept the deletion of the threat list `id` names.
    ///
    /// `None` when this detector holds no such list at all; `Some(true)` when this
    /// call is the one that accepted the deletion, and `Some(false)` when a delete
    /// had already been accepted for it — the caller needs that distinction because
    /// only the first delete changes the set of LIVE sub-resources the detector
    /// holds, and so only the first moves the detector's own `UpdatedAt`.
    ///
    /// The list moves to [`THREAT_INTEL_SET_DELETE_PENDING`] and the clock owes it
    /// [`THREAT_INTEL_SET_DELETED`], which is what `GetThreatIntelSet` reports
    /// afterwards.
    ///
    /// Keeping a deleted record is what lets a read report that status, so this is
    /// also where the retention is BOUNDED: at most
    /// [`DELETED_THREAT_INTEL_SETS_RETAINED`] deleted records survive per detector,
    /// and a delete past that drops the one deleted longest ago — the same
    /// oldest-first eviction [`Self::delete_ip_set`] uses, and for the same reason:
    /// without it a caller could create and delete without limit and make the
    /// service retain every record.
    pub fn delete_threat_intel_set(&mut self, id: &str, now: i64) -> Option<bool> {
        let accepted = self.threat_intel_sets.get_mut(id)?.begin_delete(now);
        if accepted {
            self.evict_deleted_threat_intel_sets(Some(id));
        }
        Some(accepted)
    }

    /// Drop deleted threat lists, oldest deletion first, until at most
    /// [`DELETED_THREAT_INTEL_SETS_RETAINED`] remain, so the TOTAL this detector
    /// holds is bounded at [`THREAT_INTEL_SETS_PER_DETECTOR`] live records plus that
    /// allowance however many create-and-delete cycles a caller sends. Called on
    /// BOTH write paths — the delete that leaves a record behind and the create that
    /// adds one — so neither can grow the map past that total.
    ///
    /// `keep` is never evicted: it is the record the delete that triggered this just
    /// accepted, and a caller reads that one back.
    pub fn evict_deleted_threat_intel_sets(&mut self, keep: Option<&str>) {
        let surplus = self
            .threat_intel_sets
            .values()
            .filter(|set| set.is_deleting())
            .count()
            .saturating_sub(DELETED_THREAT_INTEL_SETS_RETAINED);
        if surplus == 0 {
            return;
        }
        // The oldest deletions first: `deleted_at_millis` is the instant the delete
        // was ACCEPTED and nothing moves it afterwards, and `tick_millis` makes
        // those instants strictly increasing, so the ordering is total and the id
        // only breaks a tie that cannot arise.
        let mut deleted: Vec<(i64, String)> = self
            .threat_intel_sets
            .values()
            .filter(|set| set.is_deleting() && Some(set.id.as_str()) != keep)
            .map(|set| (set.deleted_at_millis.unwrap_or_default(), set.id.clone()))
            .collect();
        deleted.sort_unstable();
        for (_, id) in deleted.into_iter().take(surplus) {
            self.threat_intel_sets.remove(&id);
        }
    }

    /// Set one feature's status, recording `now` as its `UpdatedAt`. Adds the
    /// feature when the detector does not carry it yet.
    pub fn set_feature(&mut self, name: &str, status: &str, now: i64) {
        set_feature_in(&mut self.features, name, status, now);
    }

    /// The publishing destination a create carrying `token` already minted, if any
    /// — the `@idempotencyToken` lookup, scoped to the detector the destination
    /// hangs off, the way [`Self::threat_intel_set_for_token`] is.
    ///
    /// The token lives ON the record, so the index is bounded by
    /// [`PUBLISHING_DESTINATIONS_PER_DETECTOR`] with the destinations themselves
    /// and a mapping cannot outlive the resource it names.
    pub fn publishing_destination_for_token(
        &self,
        token: &str,
    ) -> Option<&PublishingDestinationRecord> {
        self.publishing_destinations
            .values()
            .find(|destination| destination.client_token.as_deref() == Some(token))
    }

    /// Put every named finding this detector holds into the `archived` state,
    /// recording `now` as the `UpdatedAt` of each finding the call actually moves.
    ///
    /// Archiving and unarchiving are the two directions of ONE write over the
    /// `FindingIds` a request names (`smithy_query(operation="ArchiveFindings")` /
    /// `("UnarchiveFindings")`), so both go through here and the LAST call to land
    /// decides the state. A finding already in the requested state is left
    /// untouched — `UpdatedAt` included, which is what makes a repeated call a true
    /// no-op — and an id this detector does not hold moves nothing, the same way a
    /// findings read reports only the ids it holds.
    ///
    /// The archive state is a property of the RECORD, not an observation of the
    /// activity, so only [`FindingRecord::updated_at_millis`] moves:
    /// [`FindingRecord::event_last_seen_millis`] stays where the last observation
    /// left it.
    ///
    /// Answers whether this call moved any finding at all, which is what lets the
    /// caller take the instant it stamped off the clock only when one really moved.
    pub fn set_findings_archived(
        &mut self,
        finding_ids: &[String],
        archived: bool,
        now: i64,
    ) -> bool {
        let mut moved = false;
        for id in finding_ids {
            if let Some(finding) = self.findings.get_mut(id) {
                if finding.archived != archived {
                    finding.archived = archived;
                    finding.updated_at_millis = now;
                    moved = true;
                }
            }
        }
        moved
    }

    /// Is `finding` selected by at least one of this detector's SUPPRESSION filters
    /// — a saved filter whose action archives
    /// ([`FilterRecord::archives_matching_findings`])?
    ///
    /// A filter selects findings by exactly the rule a findings read selects them by
    /// ([`finding_matches`]), so the set a filter suppresses is the set a
    /// `ListFindings` carrying its criteria reports and the two cannot drift apart.
    /// A saved filter's criteria are resolved as fields of a finding before the
    /// filter is stored, so the matcher cannot reject them here; one it still could
    /// not evaluate selects NOTHING rather than suppressing everything.
    pub fn finding_is_suppressed(&self, finding: &FindingRecord) -> bool {
        self.filters.values().any(|filter| {
            filter.archives_matching_findings()
                && finding_matches(finding, Some(&filter.finding_criteria)).unwrap_or(false)
        })
    }

    /// Apply ONE saved filter's action: when the named filter archives, put every
    /// finding this detector holds that its criteria select into the archived state,
    /// recording `now` as the `UpdatedAt` of each finding the call actually moves. A
    /// filter whose action names no change, and a name this detector does not hold,
    /// move nothing.
    ///
    /// Scoped to the filter a call just saved, so saving one filter never moves the
    /// archive state a caller set through the explicit write for findings that filter
    /// does not select. The write goes through [`Self::set_findings_archived`], the
    /// ONE archive write, so a suppressed finding is archived exactly as an explicit
    /// `ArchiveFindings` archives it and the last call to land still decides the
    /// state: a finding a caller unarchives stays unarchived until the filter is
    /// saved over it again or the activity is observed again.
    ///
    /// Answers whether this call moved any finding at all, as
    /// [`Self::set_findings_archived`] does and for the same reason.
    pub fn apply_filter_action(&mut self, filter_name: &str, now: i64) -> bool {
        let Some(filter) = self.filters.get(filter_name) else {
            return false;
        };
        if !filter.archives_matching_findings() {
            return false;
        }
        let criteria = filter.finding_criteria.clone();
        let selected: Vec<String> = self
            .findings
            .values()
            .filter(|finding| finding_matches(finding, Some(&criteria)).unwrap_or(false))
            .map(|finding| finding.id.clone())
            .collect();
        self.set_findings_archived(&selected, true, now)
    }

    /// Apply the archive action of EVERY saved suppression filter to one finding: the
    /// path that has just generated or re-observed it archives it when any standing
    /// filter selects it ([`Self::finding_is_suppressed`]), so generating a finding
    /// never moves the archive state of a finding it is not about.
    pub fn archive_if_suppressed(&mut self, finding_id: &str, now: i64) -> bool {
        let suppressed = self
            .findings
            .get(finding_id)
            .is_some_and(|finding| self.finding_is_suppressed(finding));
        suppressed && self.set_findings_archived(&[finding_id.to_string()], true, now)
    }

    /// Record `feedback` (and the `comments` it came with) on every named finding
    /// this detector holds, recording `now` as the `UpdatedAt` of each finding the
    /// call actually changes.
    ///
    /// The submission REPLACES whatever was recorded before: the feedback is one
    /// value per finding, not a log. An id this detector does not hold writes
    /// nothing and is not an error, the same way a findings read reports only the
    /// ids it holds.
    ///
    /// The feedback is a property of the RECORD, not an observation of the
    /// activity, so — exactly as for [`Self::set_findings_archived`] — only
    /// [`FindingRecord::updated_at_millis`] moves and
    /// [`FindingRecord::event_last_seen_millis`] stays where the last observation
    /// left it. A submission that leaves every stored value where it was moves
    /// nothing at all, which is what makes a re-submission of the same feedback a
    /// true no-op.
    ///
    /// Answers whether this call changed any finding at all, as
    /// [`Self::set_findings_archived`] does and for the same reason.
    pub fn set_findings_feedback(
        &mut self,
        finding_ids: &[String],
        feedback: &str,
        comments: Option<&str>,
        now: i64,
    ) -> bool {
        let mut moved = false;
        for id in finding_ids {
            if let Some(finding) = self.findings.get_mut(id) {
                let changed = finding.user_feedback.as_deref() != Some(feedback)
                    || finding.feedback_comments.as_deref() != comments;
                if changed {
                    finding.user_feedback = Some(feedback.to_string());
                    finding.feedback_comments = comments.map(str::to_string);
                    finding.updated_at_millis = now;
                    moved = true;
                }
            }
        }
        moved
    }

    /// The first id in `finding_ids` this detector holds no finding for, in the
    /// order the request listed them.
    ///
    /// The archive-state mutations resolve every id BEFORE they write
    /// ([`Self::set_findings_archived`]), so a batch naming an id the detector
    /// does not hold is refused whole and moves nothing: a caller archiving in
    /// bulk learns that one of its ids is wrong instead of having the rest
    /// silently applied. The FIRST unheld id is reported so the rejection is a
    /// function of the request alone and not of iteration order over the store.
    pub fn first_unheld_finding_id<'ids>(&self, finding_ids: &'ids [String]) -> Option<&'ids str> {
        finding_ids
            .iter()
            .find(|id| !self.findings.contains_key(id.as_str()))
            .map(String::as_str)
    }

    /// Put a pending invitation from `administrator_account_id` in this detector's
    /// inbox, reporting whether this call delivered one.
    ///
    /// The inverse of [`Self::accept_invitation`]: the record it writes is exactly
    /// the one an acceptance resolves, an inbox read reports and a decline or a
    /// delete takes out. A delivered invitation is [`RELATIONSHIP_INVITED`] — it
    /// is pending — and carries `now` as the instant it was SENT, which is the
    /// `InvitedAt` every read of it reports and the one the accepted relationship
    /// keeps.
    ///
    /// Writes NOTHING, and reports `false`, for an account that already has an
    /// invitation pending here (that invitation stays as it is, so the inviter
    /// cannot overwrite the id the invited account was handed) and for an inbox
    /// already holding [`MAX_INVITATIONS_PER_DETECTOR`] invitations from other
    /// accounts, which is the ceiling that keeps a request-keyed store bounded.
    ///
    /// The detector itself does not move: an invitation is a record of its own,
    /// not a change to the detector's configuration, so `UpdatedAt` is left where
    /// it was until an acceptance establishes the relationship.
    pub fn deliver_invitation(
        &mut self,
        administrator_account_id: &str,
        invitation_id: &str,
        now: i64,
    ) -> bool {
        if !self.inbox_has_room_for(administrator_account_id) {
            return false;
        }
        self.received_invitations.insert(
            administrator_account_id.to_string(),
            ReceivedInvitationRecord {
                administrator_account_id: administrator_account_id.to_string(),
                invitation_id: invitation_id.to_string(),
                relationship_status: RELATIONSHIP_INVITED.to_string(),
                invited_at_millis: now,
            },
        );
        true
    }

    /// Can this detector's inbox take an invitation from `administrator_account_id`?
    ///
    /// The condition [`Self::deliver_invitation`] writes under, read over an
    /// IMMUTABLE borrow so a delivery can answer it BEFORE it mints an id: an
    /// account that already has an invitation pending here keeps that invitation,
    /// and an inbox at [`MAX_INVITATIONS_PER_DETECTOR`] takes no further account.
    /// One definition, so the check and the write cannot disagree on what the
    /// inbox will accept.
    pub fn inbox_has_room_for(&self, administrator_account_id: &str) -> bool {
        !self.has_invitation_pending_from(administrator_account_id)
            && self.received_invitations.len() < MAX_INVITATIONS_PER_DETECTOR
    }

    /// Is an invitation from `administrator_account_id` pending for this detector's
    /// account, whatever its id?
    ///
    /// ONE invitation is pending per inviting account at a time (see
    /// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::deliver_invitation_in`]), so
    /// this single lookup is what "still pending" means for that account: it is the
    /// condition a delivery takes its room from ([`Self::inbox_has_room_for`]) and
    /// the condition a send leaves its member records untouched under, read over an
    /// IMMUTABLE borrow so either caller can answer it before it writes. One
    /// definition, so the two halves of a send cannot disagree about what is
    /// pending.
    pub fn has_invitation_pending_from(&self, administrator_account_id: &str) -> bool {
        self.received_invitations
            .contains_key(administrator_account_id)
    }

    /// Is an invitation pending for this detector's account under exactly the pair
    /// `(administrator_account_id, invitation_id)`?
    ///
    /// The condition [`Self::accept_invitation`] accepts on, read over an IMMUTABLE
    /// borrow so the operations bound to it can classify their refusal before they
    /// touch anything — one definition, so the check and the write cannot disagree
    /// on what is pending.
    pub fn invitation_pending(&self, administrator_account_id: &str, invitation_id: &str) -> bool {
        self.received_invitations
            .get(administrator_account_id)
            .is_some_and(|invitation| invitation.invitation_id == invitation_id)
    }

    /// Accept the invitation `(administrator_account_id, invitation_id)` names, if
    /// this detector's account has one pending under exactly that pair.
    ///
    /// Reports whether it did. Both members must match: `InvitationId` is "the
    /// value that is used to validate the administrator account to the member
    /// account" (`smithy_query(operation="AcceptInvitation")`), so an id that
    /// belongs to a different administrator's invitation validates nothing.
    ///
    /// The accepted invitation leaves the inbox — it is no longer pending — and
    /// the relationship it established becomes [`Self::administrator`], carrying
    /// the invitation's own `InvitedAt` rather than the instant of the acceptance.
    /// A request that matches nothing writes NOTHING, which is what makes the
    /// refusal leave master-account state exactly where it was.
    pub fn accept_invitation(
        &mut self,
        administrator_account_id: &str,
        invitation_id: &str,
        now: i64,
    ) -> bool {
        if !self.invitation_pending(administrator_account_id, invitation_id) {
            return false;
        }
        let Some(invitation) = self.received_invitations.remove(administrator_account_id) else {
            return false;
        };
        self.administrator = Some(AdministratorRecord {
            account_id: invitation.administrator_account_id,
            invitation_id: invitation.invitation_id,
            relationship_status: RELATIONSHIP_ENABLED.to_string(),
            invited_at_millis: invitation.invited_at_millis,
        });
        self.updated_at_millis = now;
        true
    }

    /// Drop the administrator relationship this detector's account is monitored
    /// by, reporting whether there was one to drop.
    ///
    /// The MEMBER side of the association, and the exact inverse of
    /// [`Self::accept_invitation`]: it clears [`Self::administrator`], so the
    /// administrator reads (`GetAdministratorAccount` / `GetMasterAccount`) report
    /// none afterwards. The accepted invitation is NOT put back in the inbox — "the
    /// member account details obtained from the CreateMembers API ... are retained.
    /// This is done so that the delegated administrator can invoke the
    /// InviteMembers API without the need to invoke the CreateMembers API again"
    /// (`smithy_query(query="doc",shape="DisassociateFromAdministratorAccount")`):
    /// re-association goes through a fresh invitation, which is the
    /// administrator's call to make and not this one's.
    ///
    /// Nothing else moves: the member records this detector's account administers
    /// are the OTHER side of the relationship (they belong to the accounts this one
    /// administers, not to the administrator it is leaving), so they are untouched.
    /// `false` writes NOTHING at all, which is what lets the caller refuse without
    /// having changed anything.
    ///
    /// Shared with the deprecated `DisassociateFromMasterAccount` ("this operation
    /// is deprecated, use DisassociateFromAdministratorAccount instead",
    /// `smithy_query(operation="DisassociateFromMasterAccount")`), which is still a
    /// scaffold with an issue of its own: the two names are one behaviour, so
    /// neither can drop a relationship the other would keep.
    pub fn disassociate_from_administrator(&mut self, now: i64) -> bool {
        if self.administrator.take().is_none() {
            return false;
        }
        self.updated_at_millis = now;
        true
    }

    /// The investigation a create already minted for `token`, if any — the seam
    /// that makes `CreateInvestigation` idempotent under a replayed `ClientToken`.
    pub fn investigation_for_token(&self, token: &str) -> Option<&InvestigationRecord> {
        self.investigations
            .values()
            .find(|investigation| investigation.client_token.as_deref() == Some(token))
    }

    /// The member account ids this detector administers, in account-id order.
    ///
    /// `only_associated` narrows the answer to the members that are ASSOCIATED
    /// with the administrator ([`MemberRecord::is_associated`]), which is the
    /// distinction `ListMembers` exposes: "whether to only return associated
    /// members or to return all members (including members who haven't been
    /// invited yet or have been disassociated)"
    /// (`smithy_query(query="doc",shape="ListMembersRequest",member="OnlyAssociated")`).
    pub fn member_ids(&self, only_associated: bool) -> Vec<String> {
        self.members
            .values()
            .filter(|member| !only_associated || member.is_associated())
            .map(|member| member.account_id.clone())
            .collect()
    }

    /// The names of the filters this detector holds, in filter-name order.
    ///
    /// The order is TOTAL — a filter name is unique within a detector, because it
    /// is the key the filters are stored under — which is what makes a page token
    /// mean the same thing on the call that hands it back.
    pub fn filter_names(&self) -> Vec<String> {
        self.filters.keys().cloned().collect()
    }

    /// How many investigations this detector holds that were initiated at or after
    /// `since`, in epoch milliseconds.
    pub fn investigations_started_since(&self, since: i64) -> usize {
        self.investigations
            .values()
            .filter(|investigation| investigation.started_at_millis >= since)
            .count()
    }

    /// Apply the status change the CLOCK already owes each of this detector's
    /// investigations, so a report answers what the service owed at `now` whether
    /// or not anybody polled. The counterpart of
    /// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::settle_malware_scans`] for
    /// the investigations one detector holds; every move goes through
    /// [`InvestigationRecord::settle`], so a read and a list cannot disagree about
    /// when an investigation left `RUNNING` or what its pass concluded.
    pub fn settle_investigations(&mut self, now: i64) {
        let evidence = self.finding_evidence();
        let member_accounts = self.members.len();
        for investigation in self.investigations.values_mut() {
            investigation.settle(now, &evidence, member_accounts);
        }
    }

    /// Apply what the clock owes the ONE investigation `id` names, and report it —
    /// `None` for an id this detector does not hold, which is the absence a reader
    /// refuses.
    ///
    /// The single-record counterpart of [`Self::settle_investigations`], through the
    /// same [`InvestigationRecord::settle`], so a read moves exactly the record it
    /// reports and no other.
    pub fn settle_investigation(&mut self, id: &str, now: i64) -> Option<&InvestigationRecord> {
        if !self.investigations.contains_key(id) {
            return None;
        }
        let evidence = self.finding_evidence();
        let member_accounts = self.members.len();
        if let Some(investigation) = self.investigations.get_mut(id) {
            investigation.settle(now, &evidence, member_accounts);
        }
        self.investigations.get(id)
    }

    /// This detector's findings as an analysis pass reads them
    /// ([`FindingEvidence`]) — the evidence every investigation of this detector is
    /// assessed from.
    fn finding_evidence(&self) -> Vec<FindingEvidence> {
        self.findings.values().map(FindingEvidence::of).collect()
    }

    /// Every investigation this detector holds, in investigation-id order — the
    /// total order the map is keyed by, which a sort then re-orders and a page
    /// token stays meaningful against.
    pub fn investigation_records(&self) -> Vec<&InvestigationRecord> {
        self.investigations.values().collect()
    }

    /// The trusted entity set a create already minted for `token`, if any — the
    /// seam that makes `CreateTrustedEntitySet` idempotent under a replayed
    /// `ClientToken`.
    ///
    /// A set whose delete has been accepted ([`EntitySetRecord::is_deleting`])
    /// resolves to nothing here, so a replayed create mints a new set rather than
    /// answering with one the service is no longer keeping — the same rule
    /// [`Self::ip_set_for_token`] follows for a deleted IPSet, whose retained
    /// record it filters out too.
    pub fn trusted_entity_set_for_token(&self, token: &str) -> Option<&EntitySetRecord> {
        self.trusted_entity_sets
            .values()
            .find(|set| set.client_token.as_deref() == Some(token) && !set.is_deleting())
    }

    /// The LIVE trusted entity set this detector holds, if any.
    ///
    /// "At any given time, you can have only one trusted entity set"
    /// (`smithy_query(query="doc",shape="CreateTrustedEntitySet")`) bounds the sets
    /// GuardDuty is keeping, so a set whose delete has been accepted is not one of
    /// them: `DeleteTrustedEntitySet` frees the detector's single slot the instant
    /// it answers, even though its record is still RETAINED
    /// ([`DELETED_ENTITY_SETS_RETAINED`]) for the family's own read to report.
    pub fn live_trusted_entity_set(&self) -> Option<&EntitySetRecord> {
        self.trusted_entity_sets
            .values()
            .find(|set| !set.is_deleting())
    }

    /// The ids of the LIVE trusted entity sets this detector holds, in
    /// `TrustedEntitySetId` order (the id is the key they are stored under, so the
    /// order is TOTAL — what makes a page token mean the same thing on the call
    /// that handed it back).
    ///
    /// A set whose delete has been accepted is named by nothing: it is no longer
    /// one of the detector's sets, so `ListTrustedEntitySets` stops naming it the
    /// instant its delete is served, while its retained record stays readable on
    /// its own id — being readable is not the same as being part of the collection
    /// a list enumerates, which is the rule [`Self::live_ip_set_ids`] follows.
    ///
    /// It reports a LIST and not the single [`Self::live_trusted_entity_set`]
    /// because the one-at-a-time rule bounds what a create may add, not what the
    /// listing is: the answer is every live record the map holds.
    pub fn live_trusted_entity_set_ids(&self) -> Vec<String> {
        self.trusted_entity_sets
            .values()
            .filter(|set| !set.is_deleting())
            .map(|set| set.id.clone())
            .collect()
    }

    /// Accept the deletion of the trusted entity set `id` names.
    ///
    /// `None` when this detector holds no such set at all, which is the absence its
    /// delete refuses; `Some(true)` when this call is the one that accepted the
    /// deletion — which is also when the detector's own `UpdatedAt` moves, because
    /// that is the call that changed the sub-resources it holds — and `Some(false)`
    /// when a delete had already been accepted for it. The shared entity-set
    /// deletion ([`delete_entity_set`]) is what stamps the status and bounds the
    /// records retained.
    pub fn delete_trusted_entity_set(&mut self, id: &str, now: i64) -> Option<bool> {
        delete_entity_set(&mut self.trusted_entity_sets, id, now)
    }

    /// The threat entity set a create already minted for `token`, if any — the
    /// seam that makes `CreateThreatEntitySet` idempotent under a replayed
    /// `ClientToken`.
    ///
    /// Scoped to the THREAT sets: a token resolves to a resource of the type the
    /// create that carried it made, so one token replayed across the two
    /// entity-set creates never hands back the other one's id.
    ///
    /// A set whose delete has been accepted ([`EntitySetRecord::is_deleting`])
    /// resolves to nothing here — the same rule
    /// [`Self::trusted_entity_set_for_token`] follows for a deleted trusted set.
    pub fn threat_entity_set_for_token(&self, token: &str) -> Option<&EntitySetRecord> {
        self.threat_entity_sets
            .values()
            .find(|set| set.client_token.as_deref() == Some(token) && !set.is_deleting())
    }

    /// The ids of the LIVE threat entity sets this detector holds, in
    /// `ThreatEntitySetId` order (the id is the key they are stored under, so the
    /// order is TOTAL — what makes a page token mean the same thing on the call
    /// that handed it back).
    ///
    /// A set whose delete has been accepted is named by nothing: it is no longer
    /// one of the detector's sets, so `ListThreatEntitySets` stops naming it the
    /// instant its delete is served, while its retained record stays readable on
    /// its own id.
    pub fn live_threat_entity_set_ids(&self) -> Vec<String> {
        self.threat_entity_sets
            .values()
            .filter(|set| !set.is_deleting())
            .map(|set| set.id.clone())
            .collect()
    }

    /// How many LIVE threat entity sets this detector holds.
    ///
    /// This is the count [`THREAT_ENTITY_SETS_PER_DETECTOR`] bounds: a set whose
    /// delete has been accepted is only reported, never used, so it occupies none
    /// of the detector's slots — the same rule [`Self::live_ip_set_count`] follows
    /// for a deleted IPSet's retained record.
    pub fn live_threat_entity_set_count(&self) -> usize {
        self.threat_entity_sets
            .values()
            .filter(|set| !set.is_deleting())
            .count()
    }

    /// Accept the deletion of the threat entity set `id` names.
    ///
    /// `None` when this detector holds no such set at all, which is the absence its
    /// delete refuses; `Some(true)` when this call is the one that accepted the
    /// deletion — which is also when the detector's own `UpdatedAt` moves — and
    /// `Some(false)` when a delete had already been accepted for it. The shared
    /// entity-set deletion ([`delete_entity_set`]) is what stamps the status and
    /// bounds the records retained.
    pub fn delete_threat_entity_set(&mut self, id: &str, now: i64) -> Option<bool> {
        delete_entity_set(&mut self.threat_entity_sets, id, now)
    }

    /// The IPSet a create already minted for `token`, if any — the seam that makes
    /// `CreateIPSet` idempotent under a replayed `ClientToken`.
    ///
    /// An IPSet a delete has already accepted ([`IpSetRecord::is_deleting`])
    /// resolves to nothing, so a replayed create never answers with a list the
    /// service is no longer keeping — the same rule
    /// [`Self::detector_for_client_token`]'s mapping follows for a deleted detector.
    pub fn ip_set_for_token(&self, token: &str) -> Option<&IpSetRecord> {
        self.ip_sets
            .values()
            .find(|ip_set| ip_set.client_token.as_deref() == Some(token) && !ip_set.is_deleting())
    }

    /// The ids of the LIVE IPSets this detector holds, in `IpSetId` order.
    ///
    /// The order is TOTAL — the id is the key the IPSets are stored under, so it is
    /// unique within a detector — which is what makes a page token mean the same
    /// thing on the call that hands it back.
    ///
    /// LIVE is the same set [`Self::live_ip_set_count`] counts: a list whose delete
    /// has been accepted is no longer one of the detector's IPSets, so `ListIPSets`
    /// stops naming it. Its record is still retained
    /// ([`DELETED_IP_SETS_RETAINED`]) so `GetIPSet` can report the
    /// [`IP_SET_DELETE_PENDING`] / [`IP_SET_DELETED`] status the delete moved it
    /// to — being readable on its own id is not the same as being part of the
    /// collection a list enumerates.
    pub fn live_ip_set_ids(&self) -> Vec<String> {
        self.ip_sets
            .values()
            .filter(|ip_set| !ip_set.is_deleting())
            .map(|ip_set| ip_set.id.clone())
            .collect()
    }

    /// How many of this detector's IPSets are LIVE — held and not being deleted.
    ///
    /// This is the count [`IP_SETS_PER_DETECTOR`] bounds: a list whose delete has
    /// been accepted is only reported, never used, so it no longer occupies one of
    /// the detector's slots.
    pub fn live_ip_set_count(&self) -> usize {
        self.live_ip_set_count_excluding(None)
    }

    /// How many of this detector's IPSets are LIVE — held and not being deleted —
    /// other than the one `except` names.
    ///
    /// This is the count [`ACTIVATED_IP_SETS_PER_ACCOUNT`] bounds when a request
    /// asks GuardDuty to start USING a list: the allowance is for the lists the
    /// account keeps, so a list whose delete has been accepted releases the one it
    /// held, and `except` is how a request about one list asks the question about
    /// the OTHERS.
    pub fn live_ip_set_count_excluding(&self, except: Option<&str>) -> usize {
        self.ip_sets
            .values()
            .filter(|ip_set| {
                !ip_set.is_deleting() && except.is_none_or(|id| ip_set.id != id)
            })
            .count()
    }

    /// Accept the deletion of the IPSet `ip_set_id` names.
    ///
    /// `None` when this detector holds no such IPSet at all; `Some(true)` when this
    /// call is the one that accepted the deletion, and `Some(false)` when a delete
    /// had already been accepted for it — the caller needs that distinction because
    /// only the first delete changes the set of LIVE sub-resources the detector
    /// holds, and so only the first moves the detector's own `UpdatedAt`.
    ///
    /// The list moves to [`IP_SET_DELETE_PENDING`] and the clock owes it
    /// [`IP_SET_DELETED`], which is what `GetIPSet` reports afterwards; a list
    /// already being deleted is left exactly where it is, so a repeated delete is a
    /// true no-op.
    ///
    /// Keeping a deleted record is what lets a read report [`IP_SET_DELETED`], so
    /// this is also where that retention is BOUNDED: at most
    /// [`DELETED_IP_SETS_RETAINED`] deleted records survive per detector, and a
    /// delete past that drops the one deleted longest ago — the same
    /// oldest-first eviction [`ComAmazonawsGuarddutyGuarddutyapiserviceState::record_client_token`]
    /// uses, for the same reason: without it a caller could create and delete
    /// without limit and make the service retain every record.
    pub fn delete_ip_set(&mut self, ip_set_id: &str, now: i64) -> Option<bool> {
        let accepted = self.ip_sets.get_mut(ip_set_id)?.begin_delete(now);
        if accepted {
            self.evict_deleted_ip_sets(ip_set_id);
        }
        Some(accepted)
    }

    /// Drop deleted IPSets, oldest first, until at most [`DELETED_IP_SETS_RETAINED`]
    /// remain. `keep` is never evicted: it is the record the call that triggered this
    /// just deleted, and a caller reads that one back.
    pub(super) fn evict_deleted_ip_sets(&mut self, keep: &str) {
        let surplus = self
            .ip_sets
            .values()
            .filter(|ip_set| ip_set.is_deleting())
            .count()
            .saturating_sub(DELETED_IP_SETS_RETAINED);
        if surplus == 0 {
            return;
        }
        // The oldest deletions first: `updated_at_millis` on a deleted record is the
        // instant its delete was accepted, and `tick_millis` makes those instants
        // strictly increasing, so the ordering is total and the id only breaks a tie
        // that cannot arise.
        let mut deleted: Vec<(i64, String)> = self
            .ip_sets
            .values()
            .filter(|ip_set| ip_set.is_deleting() && ip_set.id != keep)
            .map(|ip_set| (ip_set.updated_at_millis, ip_set.id.clone()))
            .collect();
        deleted.sort_unstable();
        for (_, id) in deleted.into_iter().take(surplus) {
            self.ip_sets.remove(&id);
        }
    }
}
