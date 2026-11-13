// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-fms-awsfms-20180101 service.
//!
//! Firewall Manager's control plane is organisation-scoped: ONE administrator
//! account association, and a flat set of policies keyed by the `PolicyId` the
//! service mints. Both live here so every operation reads and writes the same
//! store; the helpers below are the bits of derivation several handlers share
//! (the request's Region, the policy ARN, the `PolicySummary` projection and the
//! pagination token codec and page-size rule), kept in one place so two handlers
//! cannot disagree.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AccountScope, AdminAccountSummary, AdminScope, AppsListData, AppsListDataSummary,
    ComplianceViolator,
    DiscoveredResource, EvaluationResult, InvalidOperationException, OrganizationalUnitScope,
    Policy, PolicyComplianceDetail, PolicyComplianceStatus,
    PolicySummary, PolicyTypeScope, ProtocolsListData, ProtocolsListDataSummary, RegionScope,
    ResourceNotFoundException, ResourceSet, ResourceSetSummary, ResourceViolation, Tag,
    ViolationDetail,
};

/// The Region an emulated service reports when the request names none to read:
/// an unsigned request, or one whose SigV4 credential scope carries a Region
/// that is not a Region name ([`is_region_name`]).
const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace / signing name of this service, per its Smithy model
/// (`aws.api#service` arnNamespace `fms`).
const ARN_NAMESPACE: &str = "fms";

/// The status a pending transition `(settles_to, settles_at)` makes due AT
/// `now`, or `None` while nothing is owed yet.
///
/// The ONE place the "is the transition the clock owes due?" test lives: a
/// transition is due from its instant onwards, so a reader at exactly that
/// instant already sees the terminal status. Every record type that carries a
/// pending transition — and the derived read side as well as the write side of
/// each — decides from here, so no two of them can disagree about when a
/// transition has come due.
pub fn pending_transition_due(
    settles_to: &Option<String>,
    settles_at: Option<SystemTime>,
    now: SystemTime,
) -> Option<&str> {
    match (settles_to, settles_at) {
        (Some(settled), Some(at)) if now >= at => Some(settled.as_str()),
        _ => None,
    }
}

/// Materialise the clock-due transition on the `(status, settles_to,
/// settles_at)` triple a record carries: once the transition is due
/// ([`pending_transition_due`]) the status IS the one it settles to, and the
/// pending pair is cleared because nothing is owed any more. Answers whether a
/// transition was due, so a record with a side effect of its own to apply can
/// see that it settled.
///
/// The ONE copy of the rule, which every record type carrying such a pair
/// delegates to: writing it once is what keeps the clock-due transition of one
/// record type from drifting away from another's.
pub fn settle_pending_transition(
    status: &mut String,
    settles_to: &mut Option<String>,
    settles_at: &mut Option<SystemTime>,
    now: SystemTime,
) -> bool {
    let Some(due) = pending_transition_due(settles_to, *settles_at, now) else {
        return false;
    };
    *status = due.to_string();
    *settles_to = None;
    *settles_at = None;
    true
}

/// The Firewall Manager administrator account association.
///
/// `role_status` is an `AccountRoleStatus` value (see
/// [`crate::ops::types::ACCOUNT_ROLE_STATUS_VALUES`]); the association is
/// returned ready, so it is `READY` from the moment it is written.
///
/// `associated_at` is recorded here but reported by no operation: no modelled
/// member of `GetAdminAccountResponse` or `AdminAccountSummary` carries when the
/// association was made, so it is held for the operations that come to need it
/// rather than being a derivation a reader would have to compute.
///
/// `removal_due` is the instant a requested disassociation completes — see
/// [`ComAmazonawsFmsAwsfms20180101State::settle_admin_account`]. It is `None`
/// for an association no caller has asked to tear down.
#[derive(Debug, Clone)]
pub struct AdminAccountRecord {
    pub account_id: String,
    pub role_status: String,
    pub associated_at: SystemTime,
    pub removal_due: Option<SystemTime>,
}

impl AdminAccountRecord {
    /// Whether this association still makes its account the organisation's
    /// default administrator AT `now`.
    ///
    /// A `DELETED` role is the terminal state of a completed disassociation: the
    /// record is kept so `GetAdminAccount` can report what became of the account,
    /// but the administrator slot is free again. A disassociation whose
    /// completion instant ([`Self::removal_due`]) has passed is already the same
    /// thing — the transition is owed whether or not anybody has materialised it
    /// — so this predicate is derived from the CLOCK rather than from waiting for
    /// a writer, and no reader of the store can disagree with another about
    /// whether an administrator exists.
    /// [`ComAmazonawsFmsAwsfms20180101State::settle_admin_account_at`] is what
    /// persists the status that follows from it.
    ///
    /// It takes `now` so one request decides from ONE reading of a clock that is
    /// not monotonic: an operation that checks the association and then writes to
    /// it must not be able to see it active and then gone.
    pub fn is_active_at(&self, now: SystemTime) -> bool {
        self.role_status != ROLE_STATUS_DELETED && self.removal_due.is_none_or(|due| now < due)
    }

    /// Whether a disassociation of this association has been requested and has
    /// not completed yet.
    pub fn removal_pending(&self) -> bool {
        self.removal_due.is_some()
    }
}

/// `AccountRoleStatus` of an administrator account whose role exists and is
/// usable. One of [`crate::ops::types::ACCOUNT_ROLE_STATUS_VALUES`].
pub const ROLE_STATUS_READY: &str = "READY";

/// `AccountRoleStatus` of an administrator account whose role deletion has
/// been requested (by `DisassociateAdminAccount`) and has not completed yet. One
/// of [`crate::ops::types::ACCOUNT_ROLE_STATUS_VALUES`].
pub const ROLE_STATUS_PENDING_DELETION: &str = "PENDING_DELETION";

/// `AccountRoleStatus` of an administrator account whose role is being deleted:
/// the other non-terminal status of a tear-down in flight. One of
/// [`crate::ops::types::ACCOUNT_ROLE_STATUS_VALUES`].
pub const ROLE_STATUS_DELETING: &str = "DELETING";

/// `AccountRoleStatus` of an administrator account whose role deletion has
/// completed: the account is no longer the default administrator. One of
/// [`crate::ops::types::ACCOUNT_ROLE_STATUS_VALUES`].
pub const ROLE_STATUS_DELETED: &str = "DELETED";

/// How long a requested disassociation stays `PENDING_DELETION` before the role
/// is `DELETED`.
///
/// A CHOSEN span, not a documented one: neither the model nor the Firewall
/// Manager documentation states how long tearing the administrator role down
/// takes, only that the role passes through the non-terminal
/// `PENDING_DELETION` / `DELETING` statuses of `AccountRoleStatus` on the way.
/// It is deliberately non-zero, so a caller reading the association straight
/// after asking for the disassociation still observes that transient status,
/// and short enough that a caller that waits does not wait long.
pub const DISASSOCIATION_SPAN: Duration = Duration::from_millis(500);

/// `OrganizationStatus` of an administrator account the service is onboarding
/// as a Firewall Manager administrator ("the account is onboarding to Firewall
/// Manager as an administrator", `AdminAccountSummary$Status`). One of
/// [`crate::ops::types::ORGANIZATION_STATUS_VALUES`].
pub const ORGANIZATION_STATUS_ONBOARDING: &str = "ONBOARDING";

/// `OrganizationStatus` of an administrator account that is onboarded and "can
/// perform actions on the resources defined in their AdminScope"
/// (`AdminAccountSummary$Status`). One of
/// [`crate::ops::types::ORGANIZATION_STATUS_VALUES`].
pub const ORGANIZATION_STATUS_ONBOARDING_COMPLETE: &str = "ONBOARDING_COMPLETE";

/// `OrganizationStatus` of an administrator account that "is being removed as
/// an Firewall Manager administrator" (`AdminAccountSummary$Status`). One of
/// [`crate::ops::types::ORGANIZATION_STATUS_VALUES`].
pub const ORGANIZATION_STATUS_OFFBOARDING: &str = "OFFBOARDING";

/// `OrganizationStatus` of an administrator account that "has been removed as
/// an Firewall Manager administrator" (`AdminAccountSummary$Status`). One of
/// [`crate::ops::types::ORGANIZATION_STATUS_VALUES`].
pub const ORGANIZATION_STATUS_OFFBOARDING_COMPLETE: &str = "OFFBOARDING_COMPLETE";

/// How long an administrator account `PutAdminAccount` onboarded stays
/// `ONBOARDING` before the clock makes `ONBOARDING_COMPLETE` due.
///
/// A CHOSEN span, not a documented one: Firewall Manager publishes no duration
/// for onboarding an administrator, only the non-terminal `ONBOARDING` status
/// its `AdminAccountSummary$Status` passes through on the way. It is
/// deliberately NON-ZERO and inside the emulator's 250-750 ms band, so a caller
/// that reads the administrator straight after the call still observes that
/// transient status, and short enough that a caller that waits does not wait
/// long. The due instant is STORED on the record
/// ([`OrganizationAdminRecord::settles_at`]); nothing sleeps for it.
pub const ADMIN_ONBOARDING_SPAN: Duration = Duration::from_millis(500);

/// How long an administrator account that has asked to be disassociated stays
/// `OFFBOARDING` before the clock makes `OFFBOARDING_COMPLETE` due.
///
/// A CHOSEN span, not a documented one, for the same reason
/// [`ADMIN_ONBOARDING_SPAN`] is: Firewall Manager publishes no duration for
/// removing an administrator, only the non-terminal `OFFBOARDING` status its
/// `AdminAccountSummary$Status` passes through on the way to
/// `OFFBOARDING_COMPLETE`. It is deliberately NON-ZERO, so a caller that reads
/// the administrator straight after asking for the removal still observes that
/// transient status, and short enough that a caller that waits does not wait
/// long. The due instant is STORED on the record
/// ([`OrganizationAdminRecord::settles_at`]); nothing sleeps for it.
pub const ADMIN_OFFBOARDING_SPAN: Duration = Duration::from_millis(500);

/// One Firewall Manager administrator account the organization's management
/// account onboarded with `PutAdminAccount`, keyed in
/// [`ComAmazonawsFmsAwsfms20180101State::organization_admins`] by its account
/// id.
///
/// This is the administrator that HAS A SCOPE: `PutAdminAccount` "creates or
/// updates an Firewall Manager administrator account" and configures "the
/// resources that the specified Firewall Manager administrator can manage"
/// (`PutAdminAccountRequest$AdminScope`), and `GetAdminScope` reports that
/// scope back. The organization's ONE DEFAULT administrator is the separate
/// [`AdminAccountRecord`] association `AssociateAdminAccount` makes: that one
/// "has full administrative scope that allows administration of all policy
/// types, accounts, organizational units, and Regions"
/// (`AssociateAdminAccount`), so its scope follows from being the default
/// administrator and is derived ([`default_admin_scope`]) rather than stored.
///
/// `settles_to` / `settles_at` carry the transition the CLOCK owes, exactly as
/// [`ThirdPartyFirewallRecord`] does: an account being onboarded reaches
/// `ONBOARDING_COMPLETE` [`ADMIN_ONBOARDING_SPAN`] after the call that started
/// it, whether or not anybody asks. Readers materialise it with [`Self::settle`],
/// which is the shared [`settle_pending_transition`] rule applied to this
/// record's own triple.
///
/// `onboarded_at` / `updated_at` are recorded but reported by no operation: no
/// modelled member of `AdminAccountSummary` or `GetAdminScopeResponse` carries
/// them, so they are held for the operations that come to need them.
#[derive(Debug, Clone)]
pub struct OrganizationAdminRecord {
    pub account_id: String,
    pub admin_scope: AdminScope,
    pub status: String,
    pub settles_to: Option<String>,
    pub settles_at: Option<SystemTime>,
    pub onboarded_at: SystemTime,
    pub updated_at: SystemTime,
}

impl OrganizationAdminRecord {
    /// A newly onboarded administrator account: `ONBOARDING` now, with the
    /// clock owing `ONBOARDING_COMPLETE` [`ADMIN_ONBOARDING_SPAN`] later.
    ///
    /// The ONE place that shape is built, so onboarding an account for the
    /// first time and onboarding one again after it has been removed as an
    /// administrator ([`Self::is_administrator_at`] false) cannot drift apart:
    /// both are the "create" half of `PutAdminAccount`'s "creates or updates".
    pub fn onboarding(account_id: &str, admin_scope: AdminScope, now: SystemTime) -> Self {
        Self {
            account_id: account_id.to_string(),
            admin_scope,
            status: ORGANIZATION_STATUS_ONBOARDING.to_string(),
            settles_to: Some(ORGANIZATION_STATUS_ONBOARDING_COMPLETE.to_string()),
            settles_at: Some(now + ADMIN_ONBOARDING_SPAN),
            onboarded_at: now,
            updated_at: now,
        }
    }

    /// The `OrganizationStatus` this administrator account has AT `now`,
    /// derived from the CLOCK: once [`Self::settles_at`] has passed the status
    /// IS [`Self::settles_to`], because that transition was owed at that instant
    /// even if nobody had called. Readers answer from here, so none of them can
    /// disagree with another about whether the onboarding has completed.
    pub fn status_at(&self, now: SystemTime) -> &str {
        pending_transition_due(&self.settles_to, self.settles_at, now)
            .unwrap_or(self.status.as_str())
    }

    /// Whether this account still IS one of the organization's Firewall Manager
    /// administrators AT `now`.
    ///
    /// An account whose `OrganizationStatus` has reached
    /// [`ORGANIZATION_STATUS_OFFBOARDING_COMPLETE`] "has been removed as an
    /// Firewall Manager administrator" (`AdminAccountSummary$Status`), so it is
    /// not one any more; every other status is a role the account still holds,
    /// including the removal that is still under way. The record is KEPT once
    /// the removal completes — exactly as the default administrator's
    /// association is kept `DELETED` — so a reader is still told what became of
    /// the account.
    ///
    /// Derived from [`Self::status_at`], so it is the CLOCK that decides it and
    /// no reader of the store can disagree with another about whether an
    /// account is still an administrator.
    pub fn is_administrator_at(&self, now: SystemTime) -> bool {
        self.status_at(now) != ORGANIZATION_STATUS_OFFBOARDING_COMPLETE
    }

    /// Whether this account's administrator role is GONE OR GOING AT `now`: the
    /// removal [`Self::start_offboarding`] started has completed
    /// ([`ORGANIZATION_STATUS_OFFBOARDING_COMPLETE`]), or it is still under way
    /// ([`ORGANIZATION_STATUS_OFFBOARDING`]) with the clock still owing the
    /// completion.
    ///
    /// Such a record holds no administrator role a writer could update: the
    /// account is being removed as one, so the only thing a writer can do with
    /// it is designate the account again ([`Self::start_onboarding`]).
    ///
    /// Derived from [`Self::status_at`], so the CLOCK decides it, exactly as
    /// [`Self::is_administrator_at`] is.
    pub fn is_leaving_at(&self, now: SystemTime) -> bool {
        let status = self.status_at(now);
        status == ORGANIZATION_STATUS_OFFBOARDING
            || status == ORGANIZATION_STATUS_OFFBOARDING_COMPLETE
    }

    /// Designate this account as an administrator AGAIN from `now`, with
    /// `admin_scope`: the record becomes a fresh [`Self::onboarding`], so the
    /// status the clock completes is the one THIS call started and nothing of
    /// the role the account used to hold carries over.
    ///
    /// A removal still in flight is SUPERSEDED rather than waited for — the
    /// mirror of [`Self::start_offboarding`] superseding an onboarding still in
    /// flight — so the completion of a removal the clock still owed can never
    /// undo the designation this call made.
    pub fn start_onboarding(&mut self, admin_scope: AdminScope, now: SystemTime) {
        let account_id = self.account_id.clone();
        *self = Self::onboarding(&account_id, admin_scope, now);
    }

    /// Start removing this account as an administrator: the status becomes
    /// `OFFBOARDING` now and the clock owes `OFFBOARDING_COMPLETE`
    /// [`ADMIN_OFFBOARDING_SPAN`] later — the removal side of the same pending
    /// transition the onboarding uses, so both settle through
    /// [`Self::settle`].
    ///
    /// An onboarding still in flight is SUPERSEDED rather than waited for: an
    /// account on its way out is being removed from that instant, so the
    /// transition the clock owed it is replaced by this one.
    ///
    /// Idempotent: an account that is already being removed, or already
    /// removed, has nothing left to start, so a second call does not push its
    /// due instant back.
    pub fn start_offboarding(&mut self, now: SystemTime) {
        let status = self.status_at(now);
        if status == ORGANIZATION_STATUS_OFFBOARDING
            || status == ORGANIZATION_STATUS_OFFBOARDING_COMPLETE
        {
            return;
        }
        self.status = ORGANIZATION_STATUS_OFFBOARDING.to_string();
        self.settles_to = Some(ORGANIZATION_STATUS_OFFBOARDING_COMPLETE.to_string());
        self.settles_at = Some(now + ADMIN_OFFBOARDING_SPAN);
        self.updated_at = now;
    }

    /// Write the clock-due transition into the record — the ONLY write a
    /// reporting operation makes, and the persisted form of what
    /// [`Self::status_at`] already derives. The rule itself is
    /// [`settle_pending_transition`], shared with every other record type that
    /// carries a pending transition.
    pub fn settle(&mut self, now: SystemTime) {
        settle_pending_transition(
            &mut self.status,
            &mut self.settles_to,
            &mut self.settles_at,
            now,
        );
    }
}

/// The `CustomerPolicyScopeIdType` key under which a policy's `IncludeMap` /
/// `ExcludeMap` holds ACCOUNT IDS (the only other key, `ORG_UNIT`, holds
/// organizational-unit ids). One of
/// [`crate::ops::types::CUSTOMER_POLICY_SCOPE_ID_TYPE_VALUES`].
pub const SCOPE_ID_TYPE_ACCOUNT: &str = "ACCOUNT";

/// The `SecurityServiceType` of a Shield Advanced policy: the ONLY policy type
/// `GetProtectionStatus` reports attack data for ("If you created a Shield
/// Advanced policy, returns policy-level attack summary information … Other
/// policy types are currently unsupported"). One of
/// [`crate::ops::types::SECURITY_SERVICE_TYPE_VALUES`].
pub const SECURITY_SERVICE_SHIELD_ADVANCED: &str = "SHIELD_ADVANCED";

/// The `CustomerPolicyStatus` of a policy that IS in the administrative scope
/// governing it: "the administrator can manage and delete the policy"
/// (`Policy$PolicyStatus`). One of
/// [`crate::ops::types::CUSTOMER_POLICY_STATUS_VALUES`].
pub const POLICY_STATUS_ACTIVE: &str = "ACTIVE";

/// The `CustomerPolicyStatus` of a policy that is OUTSIDE the administrative
/// scope governing it: "the administrator can view the policy, but they can't
/// edit or delete the policy" (`Policy$PolicyStatus`). One of
/// [`crate::ops::types::CUSTOMER_POLICY_STATUS_VALUES`].
pub const POLICY_STATUS_OUT_OF_ADMIN_SCOPE: &str = "OUT_OF_ADMIN_SCOPE";

/// The `ResourceTagLogicalOperator` a policy combines its `ResourceTags` with
/// when the request names none: the member's own documented default
/// ("Default: `AND`", `Policy$ResourceTagLogicalOperator`) — "a resource must
/// have all tags to be included or excluded". One of
/// [`crate::ops::types::RESOURCE_TAG_LOGICAL_OPERATOR_VALUES`].
pub const RESOURCE_TAG_LOGICAL_OPERATOR_AND: &str = "AND";

/// Materialise the documented defaults of a `Policy` document the caller sent,
/// so the stored policy is the EFFECTIVE one and every reader of it reports the
/// value the policy actually behaves by rather than nothing.
///
/// A modelled member whose documentation states an explicit default is not
/// "unset" when the request omits it: it has that value, which is why the
/// service answers with it. `ResourceTagLogicalOperator` is the one such member
/// of `Policy` — the others that carry a default (`ExcludeResourceTags`,
/// `RemediationEnabled`, `DeleteUnusedFMManagedResources`) state it as the
/// model's own `@default` trait, which the generated type materialises already
/// by not being nullable.
///
/// Applied by the single writer of a policy document ([`crate::ops::put_policy`])
/// BEFORE the document is stored, so the default is persisted once rather than
/// re-derived by each reader. A value the caller DID send is left exactly as
/// sent.
pub fn apply_policy_document_defaults(policy: &mut Policy) {
    if policy.resource_tag_logical_operator.is_none() {
        policy.resource_tag_logical_operator =
            Some(RESOURCE_TAG_LOGICAL_OPERATOR_AND.to_string());
    }
}

/// The `ResourceSetStatus` of a resource set that IS in the Region scope of the
/// administrative scope governing it: "the administrator can manage and delete
/// the resource set" (`ResourceSet$ResourceSetStatus`). One of
/// [`crate::ops::types::RESOURCE_SET_STATUS_VALUES`].
pub const RESOURCE_SET_STATUS_ACTIVE: &str = "ACTIVE";

/// The `ResourceSetStatus` of a resource set that is OUTSIDE the Region scope of
/// the administrative scope governing it: "the administrator can view the
/// resource set, but they can't edit or delete the resource set"
/// (`ResourceSet$ResourceSetStatus`). One of
/// [`crate::ops::types::RESOURCE_SET_STATUS_VALUES`].
pub const RESOURCE_SET_STATUS_OUT_OF_ADMIN_SCOPE: &str = "OUT_OF_ADMIN_SCOPE";

/// `ThirdPartyFirewallAssociationStatus` of a third-party firewall that was
/// never associated with this organisation. One of
/// [`crate::ops::types::THIRD_PARTY_FIREWALL_ASSOCIATION_STATUS_VALUES`].
pub const THIRD_PARTY_FIREWALL_NOT_EXIST: &str = "NOT_EXIST";

/// `ThirdPartyFirewallAssociationStatus` of an association that is being torn
/// down: the transient status a `DisassociateThirdPartyFirewall` starts.
pub const THIRD_PARTY_FIREWALL_OFFBOARDING: &str = "OFFBOARDING";

/// `ThirdPartyFirewallAssociationStatus` of an association that is being set
/// up: the transient status an `AssociateThirdPartyFirewall` starts, in which
/// the Firewall Manager administrator is not a tenant administrator of the
/// vendor yet. One of
/// [`crate::ops::types::THIRD_PARTY_FIREWALL_ASSOCIATION_STATUS_VALUES`].
pub const THIRD_PARTY_FIREWALL_ONBOARDING: &str = "ONBOARDING";

/// `ThirdPartyFirewallAssociationStatus` of a completed association: the
/// Firewall Manager administrator IS a tenant administrator of the vendor, which
/// is the only status in which the vendor's tenant — and so the firewall
/// policies it holds — is reachable. One of
/// [`crate::ops::types::THIRD_PARTY_FIREWALL_ASSOCIATION_STATUS_VALUES`].
pub const THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE: &str = "ONBOARD_COMPLETE";

/// `ThirdPartyFirewallAssociationStatus` of a torn-down association: the
/// terminal status the clock makes due after [`ASSOCIATION_SETTLE`].
pub const THIRD_PARTY_FIREWALL_OFFBOARD_COMPLETE: &str = "OFFBOARD_COMPLETE";

/// `MarketplaceSubscriptionOnboardingStatus` of an account with no Marketplace
/// subscription to a third-party firewall. One of
/// [`crate::ops::types::MARKETPLACE_SUBSCRIPTION_ONBOARDING_STATUS_VALUES`].
pub const MARKETPLACE_ONBOARDING_NO_SUBSCRIPTION: &str = "NO_SUBSCRIPTION";

/// `MarketplaceSubscriptionOnboardingStatus` of an administrator whose
/// subscription to the vendor is under way but not active yet: "in the process
/// of subscribing to the third-party firewall service in the Amazon Web Services
/// Marketplace, but doesn't yet have an active subscription"
/// (`GetThirdPartyFirewallAssociationStatusResponse$MarketplaceOnboardingStatus`).
/// One of [`crate::ops::types::MARKETPLACE_SUBSCRIPTION_ONBOARDING_STATUS_VALUES`].
pub const MARKETPLACE_ONBOARDING_NOT_COMPLETE: &str = "NOT_COMPLETE";

/// `MarketplaceSubscriptionOnboardingStatus` of an administrator that "has an
/// active subscription to the third-party firewall service in the Amazon Web
/// Services Marketplace"
/// (`GetThirdPartyFirewallAssociationStatusResponse$MarketplaceOnboardingStatus`).
/// One of [`crate::ops::types::MARKETPLACE_SUBSCRIPTION_ONBOARDING_STATUS_VALUES`].
pub const MARKETPLACE_ONBOARDING_COMPLETE: &str = "COMPLETE";

/// How long a third-party firewall association stays in a transient status
/// (`ONBOARDING` / `OFFBOARDING`) before the clock makes the terminal one due.
///
/// An EMULATOR-CHOSEN span, not an AWS-documented duration: Firewall Manager
/// publishes no settling time for an association. It is deliberately NON-ZERO
/// and inside the emulator's 250-750 ms band, so a caller that reads the status
/// straight after the call still observes the transient status, and
/// deliberately short so a test can wait it out. The due instant is STORED on
/// the record (see [`ThirdPartyFirewallRecord::settles_at`]); nothing sleeps
/// for it.
pub const ASSOCIATION_SETTLE: Duration = Duration::from_millis(400);

/// One stored Firewall Manager policy: the modelled `Policy` exactly as the
/// caller sent it (plus the identifiers the service minted), its ARN, the
/// current update token, and its tags.
///
/// `created_at` / `updated_at` are recorded but not yet reported: no modelled
/// member of `Policy` or `PolicySummary` carries them, and they are what the
/// later compliance-status slice (`ListComplianceStatus`, which reports a
/// policy's last evaluation) reads.
///
/// `region` is the Region the policy was created in — the one its ARN names.
/// It is held as a field rather than read back out of the ARN because it is
/// state in its own right: a policy is a regional resource, and whether it is
/// in an administrator's Region scope
/// ([`ComAmazonawsFmsAwsfms20180101State::policy_status`]) is decided from it.
#[derive(Debug, Clone)]
pub struct PolicyRecord {
    pub policy_id: String,
    pub policy_arn: String,
    pub region: String,
    pub update_token: String,
    pub policy: Policy,
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl PolicyRecord {
    /// The stored policy as `GetPolicy`/`PutPolicy` return it: the caller's own
    /// members, with the service-owned `PolicyId` and `PolicyUpdateToken`
    /// filled in from the record so a read never reports a stale token, and the
    /// service-derived `PolicyStatus` the caller passes in.
    ///
    /// `policy_status` is a PARAMETER rather than a stored member because the
    /// service derives it from the administrative scope governing the policy
    /// ([`ComAmazonawsFmsAwsfms20180101State::policy_status`]), which lives
    /// beside the policy rather than inside it: taking it here is what stops a
    /// reader reporting a status that is not derived at all.
    pub fn policy(&self, policy_status: &str) -> Policy {
        let mut policy = self.policy.clone();
        policy.policy_id = Some(self.policy_id.clone());
        policy.policy_update_token = Some(self.update_token.clone());
        policy.policy_status = Some(policy_status.to_string());
        policy
    }

    /// The `PolicySummary` projection `ListPolicies` reports, every member read
    /// off the stored policy (`SecurityServiceType` is the stored
    /// `SecurityServicePolicyData.Type`) and `PolicyStatus` the same derived
    /// value [`Self::policy`] reports, so a summary cannot disagree with the
    /// `Policy` a reader gets for the same id.
    pub fn summary(&self, policy_status: &str) -> PolicySummary {
        PolicySummary {
            policy_arn: Some(self.policy_arn.clone()),
            policy_id: Some(self.policy_id.clone()),
            policy_name: Some(self.policy.policy_name.clone()),
            resource_type: Some(self.policy.resource_type.clone()),
            security_service_type: Some(self.policy.security_service_policy_data.r#type.clone()),
            remediation_enabled: self.policy.remediation_enabled,
            delete_unused_fm_managed_resources: self.policy.delete_unused_fm_managed_resources,
            policy_status: Some(policy_status.to_string()),
        }
    }
}

/// The Firewall Manager SNS notification channel: the topic Firewall Manager
/// records its activity to, and the role that lets it publish there. ONE per
/// account/Region, which is why it is held in a map keyed by Region
/// ([`ComAmazonawsFmsAwsfms20180101State::notification_channels`]) rather than
/// in a single slot: an `Option` could only ever hold one channel for the whole
/// account.
///
/// The record carries no `region` member — unlike [`PolicyRecord::region`] and
/// [`ResourceSetRecord::region`], which belong to records keyed by an identifier
/// the service minted — because the channel has no identifier of its own: the
/// Region IS its key, so a field holding it could only be a second copy of the
/// key that could drift from it.
#[derive(Debug, Clone)]
pub struct NotificationChannelRecord {
    pub sns_topic_arn: String,
    pub sns_role_name: String,
    /// When `PutNotificationChannel` last wrote the channel. Recorded because
    /// the channel is a configuration with a history, though no modelled member
    /// of `GetNotificationChannelResponse` reports it.
    pub configured_at: SystemTime,
}

/// One stored Firewall Manager resource set: the modelled `ResourceSet` exactly
/// as the caller sent it (plus the identifiers the service minted), its ARN, the
/// current update token, its tags, and the resource URIs associated with it.
///
/// `resources` is a `BTreeSet`: associating a URI twice is the same association
/// (the set is the membership, not a log of calls), and the total order makes a
/// later page of `ListResourceSetResources` resumable from a key.
///
/// `region` is the Region the resource set was created in — the one its ARN
/// names. It is held as a field rather than read back out of the ARN for the
/// same reason [`PolicyRecord::region`] is: a resource set is a regional
/// resource, and whether it is in an administrator's Region scope
/// ([`ComAmazonawsFmsAwsfms20180101State::resource_set_status`]) is decided from
/// it.
#[derive(Debug, Clone)]
pub struct ResourceSetRecord {
    pub id: String,
    pub arn: String,
    pub region: String,
    pub update_token: String,
    pub resource_set: ResourceSet,
    pub tags: Vec<Tag>,
    pub resources: BTreeSet<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl ResourceSetRecord {
    /// The stored resource set as `PutResourceSet` / `GetResourceSet` report it:
    /// the caller's own members, with the service-owned `Id`, `UpdateToken` and
    /// `LastUpdateTime` filled in from the record, and the service-derived
    /// `ResourceSetStatus` the caller passes in, so a read never reports a stale
    /// token or a status the caller invented.
    ///
    /// `resource_set_status` is a PARAMETER rather than a stored member for the
    /// same reason [`PolicyRecord::policy`] takes one: the service derives it
    /// from the administrative scope governing the resource set
    /// ([`ComAmazonawsFmsAwsfms20180101State::resource_set_status`]), which lives
    /// beside the resource set rather than inside it, so taking it here is what
    /// stops a reader reporting a status that is not derived at all.
    pub fn resource_set(&self, resource_set_status: &str) -> ResourceSet {
        let mut resource_set = self.resource_set.clone();
        resource_set.id = Some(self.id.clone());
        resource_set.update_token = Some(self.update_token.clone());
        resource_set.last_update_time = Some(::aws_smithy_types::DateTime::from(self.updated_at));
        resource_set.resource_set_status = Some(resource_set_status.to_string());
        resource_set
    }

    /// Make `resources` this resource set's membership, recording `now` as the
    /// instant the set changed exactly when the membership it holds changed.
    ///
    /// The ONE writer of [`Self::resources`] after a record is created, used by
    /// both batch membership writers. Which resources a set holds is part of the
    /// set, so a call that adds or removes a member changed the set and
    /// `LastUpdateTime` — the member every reader projects from
    /// [`Self::updated_at`] — moves to that instant, exactly as a rewrite of the
    /// set's document moves it. A call that leaves the membership identical —
    /// associating a URI the set already holds, removing one it does not, or
    /// naming nothing at all — changed nothing, so the timestamp stays where it
    /// is: the membership is a SET, not a log of calls. Answers whether the
    /// membership changed.
    pub fn set_resources(&mut self, resources: BTreeSet<String>, now: SystemTime) -> bool {
        if self.resources == resources {
            return false;
        }
        self.resources = resources;
        self.updated_at = now;
        true
    }

    /// The `ResourceSetSummary` projection `ListResourceSets` reports: every
    /// member read off this record, with the same derived `ResourceSetStatus`
    /// [`Self::resource_set`] reports, so a summary cannot disagree with the
    /// `ResourceSet` a reader gets for the same id. The summary shape carries no
    /// `UpdateToken` and no `ResourceTypeList`, which is why it is a projection
    /// rather than the whole document.
    pub fn summary(&self, resource_set_status: &str) -> ResourceSetSummary {
        let resource_set = self.resource_set(resource_set_status);
        ResourceSetSummary {
            id: resource_set.id,
            name: Some(resource_set.name),
            description: resource_set.description,
            last_update_time: resource_set.last_update_time,
            resource_set_status: resource_set.resource_set_status,
        }
    }
}

/// One stored Firewall Manager applications list, keyed in
/// [`ComAmazonawsFmsAwsfms20180101State::apps_lists`] by the `ListId` the
/// service minted.
///
/// `default_list` distinguishes a Firewall Manager MANAGED list from one a
/// customer created: `GetAppsList`'s `DefaultList` member selects between the
/// two namespaces, so a customer list is not reported as a managed one. The
/// emulator seeds no managed lists.
#[derive(Debug, Clone)]
pub struct AppsListRecord {
    pub list_id: String,
    pub list_arn: String,
    pub update_token: String,
    pub default_list: bool,
    pub list: AppsListData,
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl AppsListRecord {
    /// The stored list as a reader reports it: the caller's own members with the
    /// service-owned `ListId`, `ListUpdateToken` and timestamps overlaid from
    /// the record.
    pub fn list(&self) -> AppsListData {
        let mut list = self.list.clone();
        list.list_id = Some(self.list_id.clone());
        list.list_update_token = Some(self.update_token.clone());
        list.create_time = Some(::aws_smithy_types::DateTime::from(self.created_at));
        list.last_update_time = Some(::aws_smithy_types::DateTime::from(self.updated_at));
        list
    }

    /// The `AppsListDataSummary` projection `ListAppsLists` reports: the four
    /// members that shape declares, every one of them read off this record, so
    /// an enumeration and a `GetAppsList` of the same list can never disagree
    /// about its ARN, its name or its applications.
    pub fn summary(&self) -> AppsListDataSummary {
        AppsListDataSummary {
            list_arn: Some(self.list_arn.clone()),
            list_id: Some(self.list_id.clone()),
            list_name: Some(self.list.list_name.clone()),
            apps_list: Some(self.list.apps_list.clone()),
        }
    }
}

/// One stored Firewall Manager protocols list. The protocols-list twin of
/// [`AppsListRecord`], with the same `default_list` split between managed and
/// customer lists.
#[derive(Debug, Clone)]
pub struct ProtocolsListRecord {
    pub list_id: String,
    pub list_arn: String,
    pub update_token: String,
    pub default_list: bool,
    pub list: ProtocolsListData,
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl ProtocolsListRecord {
    /// The stored list as `GetProtocolsList` reports it: the caller's own
    /// members with the service-owned `ListId`, `ListUpdateToken` and timestamps
    /// overlaid from the record.
    pub fn list(&self) -> ProtocolsListData {
        let mut list = self.list.clone();
        list.list_id = Some(self.list_id.clone());
        list.list_update_token = Some(self.update_token.clone());
        list.create_time = Some(::aws_smithy_types::DateTime::from(self.created_at));
        list.last_update_time = Some(::aws_smithy_types::DateTime::from(self.updated_at));
        list
    }

    /// The `ProtocolsListDataSummary` projection `ListProtocolsLists` reports:
    /// the four members that shape declares, every one of them read off this
    /// record, so an enumeration and a `GetProtocolsList` of the same list can
    /// never disagree about its ARN, its name or its protocols.
    pub fn summary(&self) -> ProtocolsListDataSummary {
        ProtocolsListDataSummary {
            list_arn: Some(self.list_arn.clone()),
            list_id: Some(self.list_id.clone()),
            list_name: Some(self.list.list_name.clone()),
            protocols_list: Some(self.list.protocols_list.clone()),
        }
    }
}

/// The association between this organisation and ONE third-party firewall
/// vendor, keyed in
/// [`ComAmazonawsFmsAwsfms20180101State::third_party_firewalls`] by the
/// `ThirdPartyFirewall` enum value.
///
/// `settles_at` / `settles_to` carry the transition the CLOCK owes: an
/// association that is onboarding or offboarding reaches its terminal status
/// [`ASSOCIATION_SETTLE`] after the call that started it, whether or not anybody
/// asks. Readers materialise it with [`Self::settle`], which is the shared
/// [`settle_pending_transition`] rule applied to this record's own triple;
/// nothing sleeps.
///
/// `marketplace_onboarding_status` is the vendor subscription's own status, which
/// is reported beside the association's. The emulator holds no Marketplace, so
/// there is no subscription state of its own to read: the value FOLLOWS the
/// designation this record tracks — in flight while the designation is
/// ([`MARKETPLACE_ONBOARDING_NOT_COMPLETE`]), active once it has completed
/// ([`MARKETPLACE_ONBOARDING_COMPLETE`]) — and a vendor with no record at all
/// has no subscription ([`MARKETPLACE_ONBOARDING_NO_SUBSCRIPTION`]). Tearing the
/// association down does NOT touch it: removing the administrator from the
/// vendor's tenant does not cancel a Marketplace subscription, so an offboarding
/// or offboarded association keeps the subscription status it had.
#[derive(Debug, Clone)]
pub struct ThirdPartyFirewallRecord {
    pub status: String,
    pub settles_to: Option<String>,
    pub settles_at: Option<SystemTime>,
    pub marketplace_onboarding_status: String,
}

impl ThirdPartyFirewallRecord {
    /// An association whose onboarding starts NOW: the designation is in flight,
    /// so the status is `ONBOARDING` and the clock owes `ONBOARD_COMPLETE`
    /// [`ASSOCIATION_SETTLE`] later. The subscription is in flight with it (see
    /// the type's docs).
    pub fn onboarding(now: SystemTime) -> Self {
        Self {
            status: THIRD_PARTY_FIREWALL_ONBOARDING.to_string(),
            settles_to: Some(THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE.to_string()),
            settles_at: Some(now + ASSOCIATION_SETTLE),
            marketplace_onboarding_status: MARKETPLACE_ONBOARDING_NOT_COMPLETE.to_string(),
        }
    }

    /// Apply the clock-due transition ([`settle_pending_transition`], the rule
    /// this record shares with every other one that carries a pending
    /// transition): once `settles_at` has passed, the status IS the terminal
    /// one, because the transition was owed at that instant even if nobody had
    /// called. Materialising it is the ONLY write a reporting operation makes.
    ///
    /// The subscription status is this record's own side effect of settling, so
    /// it is applied here rather than in the shared rule.
    pub fn settle(&mut self, now: SystemTime) {
        let settled = settle_pending_transition(
            &mut self.status,
            &mut self.settles_to,
            &mut self.settles_at,
            now,
        );
        if settled && self.status == THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE {
            // The designation completing is what makes the subscription
            // active; see the type's docs.
            self.marketplace_onboarding_status = MARKETPLACE_ONBOARDING_COMPLETE.to_string();
        }
    }

    /// Whether the Firewall Manager administrator is a tenant administrator of
    /// this vendor, i.e. whether the vendor's tenant is reachable at all. Only
    /// the terminal `ONBOARD_COMPLETE` status says so: in every other status the
    /// administrator is being designated, being removed, or removed already.
    pub fn is_onboarded(&self) -> bool {
        self.status == THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE
    }

    /// Start tearing the association down: the status becomes `OFFBOARDING` now
    /// and the clock owes `OFFBOARD_COMPLETE` [`ASSOCIATION_SETTLE`] later.
    ///
    /// Idempotent: an association that is already offboarding or offboarded has
    /// nothing left to start, so its due instant is not pushed back by a second
    /// call.
    pub fn start_offboarding(&mut self, now: SystemTime) {
        if self.status == THIRD_PARTY_FIREWALL_OFFBOARDING
            || self.status == THIRD_PARTY_FIREWALL_OFFBOARD_COMPLETE
        {
            return;
        }
        self.status = THIRD_PARTY_FIREWALL_OFFBOARDING.to_string();
        self.settles_to = Some(THIRD_PARTY_FIREWALL_OFFBOARD_COMPLETE.to_string());
        self.settles_at = Some(now + ASSOCIATION_SETTLE);
    }

    /// Start designating the Firewall Manager administrator as a tenant
    /// administrator of this vendor: the status becomes `ONBOARDING` now and the
    /// clock owes `ONBOARD_COMPLETE` [`ASSOCIATION_SETTLE`] later. An
    /// association that was torn down is designated again from here, which is
    /// why this resets the record rather than only its status.
    ///
    /// Idempotent: an association that is already onboarding or onboarded has
    /// nothing left to start, so its due instant is not pushed back by a second
    /// call — the mirror of [`Self::start_offboarding`].
    ///
    /// A subscription that is already active is kept: re-designating the tenant
    /// does not un-subscribe the administrator, so an active subscription never
    /// regresses to "in the process of subscribing".
    pub fn start_onboarding(&mut self, now: SystemTime) {
        if self.status == THIRD_PARTY_FIREWALL_ONBOARDING
            || self.status == THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE
        {
            return;
        }
        let subscription = self.marketplace_onboarding_status.clone();
        *self = Self::onboarding(now);
        if subscription == MARKETPLACE_ONBOARDING_COMPLETE {
            self.marketplace_onboarding_status = subscription;
        }
    }
}

/// One firewall policy that exists in ONE third-party firewall vendor's tenant:
/// the `FirewallPolicyId` / `FirewallPolicyName` pair
/// `ListThirdPartyFirewallFirewallPolicies` reports, keyed in
/// [`ComAmazonawsFmsAwsfms20180101State::third_party_firewall_policies`] by
/// [`third_party_firewall_policy_key`].
///
/// The vendor is part of the record (not only of its key) so a projection never
/// has to parse the key back apart, exactly as [`ShieldAttackRecord`] keeps the
/// `PolicyId` it is keyed under.
#[derive(Debug, Clone)]
pub struct ThirdPartyFirewallPolicyRecord {
    /// The `ThirdPartyFirewall` enum value of the vendor whose tenant holds this
    /// firewall policy. Also the first segment of the record's key, so one
    /// vendor's policies are a contiguous range.
    pub third_party_firewall: String,
    /// The vendor's own id for the firewall policy (`FirewallPolicyId`). Also
    /// the second segment of the record's key, so one vendor's policies are
    /// served in id order.
    pub firewall_policy_id: String,
    /// The vendor's own name for the firewall policy (`FirewallPolicyName`).
    pub firewall_policy_name: String,
}

impl ThirdPartyFirewallPolicyRecord {
    /// The key this record is held under in
    /// [`ComAmazonawsFmsAwsfms20180101State::third_party_firewall_policies`].
    pub fn key(&self) -> String {
        third_party_firewall_policy_key(&self.third_party_firewall, &self.firewall_policy_id)
    }

    /// The modelled `ThirdPartyFirewallFirewallPolicy` projection of this
    /// record: "the firewall policy name and ID", both read off the stored
    /// record, so an enumeration can never report an id or a name the store does
    /// not hold.
    ///
    /// The shape is named by its full path rather than imported at the top of
    /// this module, so adding it needs no edit to the shared `use` list every
    /// other slice of this service also touches.
    pub fn policy(&self) -> crate::ops::types::ThirdPartyFirewallFirewallPolicy {
        crate::ops::types::ThirdPartyFirewallFirewallPolicy {
            firewall_policy_id: Some(self.firewall_policy_id.clone()),
            firewall_policy_name: Some(self.firewall_policy_name.clone()),
        }
    }
}

/// The key one third-party firewall policy is held under in
/// [`ComAmazonawsFmsAwsfms20180101State::third_party_firewall_policies`]:
/// `<ThirdPartyFirewall>#<FirewallPolicyId>`.
///
/// The vendor comes first so one vendor's policies are a CONTIGUOUS range in the
/// map's total order ([`third_party_firewall_policy_key_prefix`] is that range's
/// lower bound), and the id follows so the range is ordered by id — which makes
/// a page boundary a key, the same way [`shield_attack_key`] does for attacks.
///
/// `#` separates them because it is in neither segment: the model's
/// `ThirdPartyFirewall` values are `[A-Z_]` only, and `FirewallPolicyId` matches
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, which excludes `#`. So no id can forge
/// another vendor's prefix.
pub fn third_party_firewall_policy_key(third_party_firewall: &str, policy_id: &str) -> String {
    format!("{third_party_firewall}#{policy_id}")
}

/// The prefix every key of ONE vendor's firewall policies carries, which is also
/// the lower bound of that vendor's range in
/// [`ComAmazonawsFmsAwsfms20180101State::third_party_firewall_policies`]. See
/// [`third_party_firewall_policy_key`].
pub fn third_party_firewall_policy_key_prefix(third_party_firewall: &str) -> String {
    format!("{third_party_firewall}#")
}

/// One stored Firewall Manager resource that carries TAGS, named by the kind of
/// resource it is and the key its record is stored under.
///
/// Firewall Manager's tagging operations take a generic `ResourceArn`, so what
/// an ARN resolves to is "some taggable resource of some kind" rather than a
/// policy: this is that resolution's answer (see
/// [`ComAmazonawsFmsAwsfms20180101State::tagged_resource_for_arn`]), and it is
/// what the tag read and the tag write then work from, so neither of them has to
/// know which kind of resource it was handed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// A policy, keyed by its `PolicyId` in
    /// [`ComAmazonawsFmsAwsfms20180101State::policies`].
    Policy(String),
    /// A resource set, keyed by its `Base62Id` `Id` in
    /// [`ComAmazonawsFmsAwsfms20180101State::resource_sets`].
    ResourceSet(String),
    /// An applications list, keyed by its `ListId` in
    /// [`ComAmazonawsFmsAwsfms20180101State::apps_lists`].
    AppsList(String),
    /// A protocols list, keyed by its `ListId` in
    /// [`ComAmazonawsFmsAwsfms20180101State::protocols_lists`].
    ProtocolsList(String),
}

/// One DDoS attack Shield Advanced recorded against a resource that a Firewall
/// Manager Shield Advanced policy protects — what `GetProtectionStatus`
/// projects into its `ProtectionData` document.
///
/// The members are the ones Firewall Manager documents the `ProtectionData`
/// document to carry ("Attack type / Account ID / ARN of the resource attacked /
/// Start time of the attack / End time of the attack (ongoing attacks will not
/// have an end time)", `GetProtectionStatusResponse$Data`), named as Shield
/// Advanced's own `AttackSummary` names them, plus the attack's id.
///
/// `ended_at` is `None` for an ONGOING attack, which is exactly what the
/// documented "ongoing attacks will not have an end time" means: the projection
/// leaves the end out and the attack is never excluded by the queried period's
/// lower bound.
#[derive(Debug, Clone)]
pub struct ShieldAttackRecord {
    /// The `PolicyId` of the Shield Advanced policy the attack was recorded
    /// under. Also the first segment of the record's key, so one policy's
    /// attacks are a contiguous range.
    pub policy_id: String,
    /// Shield Advanced's own id for the attack (`AttackSummary$AttackId`).
    pub attack_id: String,
    /// The organisation account that owns the attacked resource.
    pub account_id: String,
    /// The ARN of the resource that was attacked.
    pub resource_arn: String,
    /// The attack's vector types (`AttackSummary$AttackVectors`) — the
    /// documented "attack type".
    pub attack_vectors: Vec<String>,
    /// When the attack started. Also the second segment of the record's key, so
    /// the keys of one policy's attacks sort chronologically.
    pub started_at: SystemTime,
    /// When the attack ended, or `None` while it is still ongoing.
    pub ended_at: Option<SystemTime>,
}

impl ShieldAttackRecord {
    /// The key this record is held under in
    /// [`ComAmazonawsFmsAwsfms20180101State::shield_attacks`].
    pub fn key(&self) -> String {
        shield_attack_key(&self.policy_id, self.started_at, &self.attack_id)
    }

    /// Whether this attack falls in the period a request queried, where either
    /// bound may be absent (an absent bound does not constrain that side).
    ///
    /// An attack is IN the period when it OVERLAPS it: it started no later than
    /// the period's end, and it had not already ended when the period began. An
    /// ongoing attack ([`Self::ended_at`] is `None`) has not ended at all, so no
    /// lower bound excludes it.
    ///
    /// The bounds arrive as modelled `TimeStamp`s, so both sides are compared in
    /// [`epoch_nanos`] rather than converted to a `SystemTime` — the comparison
    /// is then exact and total for every instant a `TimeStamp` can carry,
    /// including one before the Unix epoch.
    pub fn overlaps(
        &self,
        start: Option<::aws_smithy_types::DateTime>,
        end: Option<::aws_smithy_types::DateTime>,
    ) -> bool {
        let started = epoch_nanos(::aws_smithy_types::DateTime::from(self.started_at));
        end.is_none_or(|end| started <= epoch_nanos(end))
            && start.is_none_or(|start| {
                self.ended_at.is_none_or(|ended| {
                    epoch_nanos(::aws_smithy_types::DateTime::from(ended)) >= epoch_nanos(start)
                })
            })
    }

    /// This attack as one entry of the `ProtectionData` JSON document
    /// `GetProtectionStatus` reports.
    ///
    /// Timestamps are Unix time in SECONDS, which is the form Firewall Manager's
    /// own `TimeStamp` members document as their default ("the default used by
    /// Firewall Manager is Unix time in seconds"). An ongoing attack carries no
    /// `EndTime` member at all rather than a null one.
    pub fn protection_data_entry(&self) -> ::serde_json::Value {
        let mut entry = ::serde_json::Map::new();
        entry.insert(
            "AttackId".to_string(),
            ::serde_json::Value::from(self.attack_id.clone()),
        );
        entry.insert(
            "AccountId".to_string(),
            ::serde_json::Value::from(self.account_id.clone()),
        );
        entry.insert(
            "ResourceArn".to_string(),
            ::serde_json::Value::from(self.resource_arn.clone()),
        );
        entry.insert(
            "AttackVectors".to_string(),
            ::serde_json::Value::from(self.attack_vectors.clone()),
        );
        entry.insert(
            "StartTime".to_string(),
            ::serde_json::Value::from(epoch_seconds(self.started_at)),
        );
        if let Some(ended_at) = self.ended_at {
            entry.insert(
                "EndTime".to_string(),
                ::serde_json::Value::from(epoch_seconds(ended_at)),
            );
        }
        ::serde_json::Value::Object(entry)
    }
}

/// `when` as Unix time in whole seconds.
///
/// `::aws_smithy_types::DateTime` is what does the conversion, so an instant
/// reported in a JSON document and one serialized into a modelled `TimeStamp`
/// member cannot disagree.
pub fn epoch_seconds(when: SystemTime) -> i64 {
    ::aws_smithy_types::DateTime::from(when).secs()
}

/// `when` as nanoseconds since the Unix epoch: the total order instants are
/// compared in, exact for every instant a modelled `TimeStamp` can carry (an
/// `i128` cannot overflow on the `i64` seconds a `DateTime` holds) and
/// infallible for one before the epoch, which converting to a `SystemTime`
/// is not.
pub fn epoch_nanos(when: ::aws_smithy_types::DateTime) -> i128 {
    i128::from(when.secs()) * 1_000_000_000 + i128::from(when.subsec_nanos())
}

/// The key one recorded attack is held under in
/// [`ComAmazonawsFmsAwsfms20180101State::shield_attacks`]: the policy it was
/// recorded under, then the instant it started, then its own id.
///
/// The segments are joined by `#`, which no `PolicyId` can contain (the model's
/// `com.amazonaws.fms#PolicyId` is `^[a-z0-9A-Z-]{36}$`), so
/// `<policy id>#` is an unambiguous prefix of exactly that policy's attacks.
/// The start instant is zero-padded Unix time, so the lexicographic order of the
/// keys within one policy IS chronological order — which makes a page boundary a
/// key, the way every other paginated read of this service resumes.
///
/// An instant before the Unix epoch sorts as the epoch itself: Shield Advanced
/// did not exist then, and ordering is the only thing the key decides (the
/// record keeps the real instant).
pub fn shield_attack_key(policy_id: &str, started_at: SystemTime, attack_id: &str) -> String {
    let since_epoch = started_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{policy_id}#{:020}.{:09}#{attack_id}",
        since_epoch.as_secs(),
        since_epoch.subsec_nanos()
    )
}

/// The prefix every key of `policy_id`'s attacks starts with — the lower bound
/// of that policy's contiguous range in
/// [`ComAmazonawsFmsAwsfms20180101State::shield_attacks`]. See
/// [`shield_attack_key`].
pub fn shield_attack_key_prefix(policy_id: &str) -> String {
    format!("{policy_id}#")
}

/// Whether `key` is a key [`shield_attack_key`] writes for an attack recorded
/// under `policy_id`.
///
/// The key's own FORM is what decides, so the value is taken apart into the
/// instant and the attack id it names and then written back through that same
/// function — nothing else is a key of this policy's range, and the check cannot
/// drift from the writer. The split is at the FIRST separator after the instant,
/// which leaves any further one with the attack id; the round trip is what
/// decides, so a key of this policy's range is accepted however its id reads.
fn is_shield_attack_key(policy_id: &str, key: &str) -> bool {
    let Some((instant, attack_id)) = key
        .strip_prefix(&shield_attack_key_prefix(policy_id))
        .and_then(|rest| rest.split_once('#'))
    else {
        return false;
    };
    let Some((seconds, nanos)) = instant.split_once('.') else {
        return false;
    };
    let started_at = seconds
        .parse()
        .ok()
        .and_then(|seconds| SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds)))
        .zip(nanos.parse().ok())
        .and_then(|(seconds, nanos)| seconds.checked_add(Duration::from_nanos(nanos)));
    started_at.is_some_and(|started_at| shield_attack_key(policy_id, started_at, attack_id) == key)
}

/// One bound of the queried period as a page stamp writes it: the instant's
/// exact nanosecond count ([`epoch_nanos`]), so two requests naming the SAME
/// instant in different `TimeStamp` formats select — and stamp — one page. An
/// absent bound, which constrains that side not at all, writes as the empty
/// string, a form no instant writes.
fn period_bound_selector(bound: Option<::aws_smithy_types::DateTime>) -> String {
    bound.map_or_else(String::new, |bound| epoch_nanos(bound).to_string())
}

/// The page scope a boundary of ONE page of a policy's recorded attacks is
/// stamped with: the four request members that SELECT the attacks that page
/// reports — the policy they were recorded under, the member account whose
/// attacked resources are reported, and the two bounds of the queried period.
///
/// All four are needed, and no subset of them identifies the page: a different
/// account's resources were attacked at different instants, and a different
/// period holds a different stretch of the inventory, so an attack of one
/// selection may be absent from another or sort after attacks that other
/// selection does hold. None of the four can contain the separator
/// [`scoped_page_key`] joins with — a `PolicyId` is `^[a-z0-9A-Z-]{36}$`, an
/// `AWSAccountId` is `^[0-9]+$`, and a bound writes as a signed integer — so the
/// stamp is unambiguous. An absent member is the empty string, which none of
/// them can carry.
fn shield_attack_page_scope(
    policy_id: &str,
    member_account_id: Option<&str>,
    start: Option<::aws_smithy_types::DateTime>,
    end: Option<::aws_smithy_types::DateTime>,
) -> String {
    scoped_page_key(
        policy_id,
        &scoped_page_key(
            member_account_id.unwrap_or_default(),
            &scoped_page_key(
                &period_bound_selector(start),
                &period_bound_selector(end),
            ),
        ),
    )
}

/// Encode a page boundary (the store key of the attack the next page resumes at)
/// of the attacks ONE `GetProtectionStatus` request selects, as an opaque
/// `PaginationToken`.
///
/// The stamp is what lets [`decode_shield_attack_page_token`] tell a boundary of
/// THIS selection's page from one minted for another member account or another
/// queried period; the encoding is [`encode_scoped_page_token`]'s, so these
/// tokens stay inside the modelled `PaginationToken` pattern and length and stay
/// opaque to a caller.
pub fn encode_shield_attack_page_token(
    policy_id: &str,
    member_account_id: Option<&str>,
    start: Option<::aws_smithy_types::DateTime>,
    end: Option<::aws_smithy_types::DateTime>,
    key: &str,
) -> String {
    encode_scoped_page_token(
        &shield_attack_page_scope(policy_id, member_account_id, start, end),
        key,
    )
}

/// Decode a `PaginationToken` produced by
/// [`encode_shield_attack_page_token`] for the attacks the policy, the member
/// account and the queried period select, yielding the whole store key the next
/// page of that selection resumes at.
///
/// `None` when the token was not produced by this service, or was produced for
/// ANOTHER selection of the inventory: an attack of another selection's page is
/// either absent from this one or sorts past attacks this one does hold, so
/// resuming from it would silently drop them and report a short page — or an
/// empty one — as a complete page. The policy alone does NOT identify this page,
/// and neither would the policy's key prefix: the other listing this service
/// narrows by a `PolicyId` (`ListComplianceStatus`) stamps its boundaries with
/// the very same policy ([`encode_scoped_page_token`]), and the same policy's
/// attacks are a different page under every account and period.
///
/// The decoded boundary must still be a key of this policy's attacks
/// ([`is_shield_attack_key`]), the shape check this page class adds on top of
/// the shared token-to-key step.
pub fn decode_shield_attack_page_token(
    policy_id: &str,
    member_account_id: Option<&str>,
    start: Option<::aws_smithy_types::DateTime>,
    end: Option<::aws_smithy_types::DateTime>,
    token: &str,
) -> Option<String> {
    let decoded = decode_scoped_page_token(
        &shield_attack_page_scope(policy_id, member_account_id, start, end),
        token,
    )?;
    is_shield_attack_key(policy_id, &decoded).then_some(decoded)
}

/// `PolicyComplianceStatusType` of a member account none of whose resources
/// violates the policy. One of
/// [`crate::ops::types::POLICY_COMPLIANCE_STATUS_TYPE_VALUES`].
pub const POLICY_COMPLIANCE_COMPLIANT: &str = "COMPLIANT";

/// `PolicyComplianceStatusType` of a member account that owns at least one
/// resource which violates the policy. One of
/// [`crate::ops::types::POLICY_COMPLIANCE_STATUS_TYPE_VALUES`].
pub const POLICY_COMPLIANCE_NON_COMPLIANT: &str = "NON_COMPLIANT";

/// The number of noncompliant resources an evaluation reports without flagging
/// that it ran out of room: `EvaluationResult$EvaluationLimitExceeded`
/// "indicates that over 100 resources are noncompliant with the Firewall Manager
/// policy", so MORE than this many violators in one account sets it. A
/// DOCUMENTED bound, from that member's own documentation.
pub const EVALUATION_VIOLATOR_LIMIT: i64 = 100;

/// One resource a Firewall Manager policy evaluation found to violate the
/// policy, in one member account of the organisation — what the compliance
/// summary (`ListComplianceStatus`) counts per account.
///
/// Like [`ComAmazonawsFmsAwsfms20180101State::shield_attacks`], the map these
/// records live in is an INVENTORY, not a cache: Firewall Manager learns a
/// violation by
/// evaluating the member accounts' resources against the policy, this emulator
/// stands in for no such evaluator, and so the inventory holds exactly what
/// something put in it. An account with nothing recorded against it is reported
/// `COMPLIANT` because no violating resource of its is known — never because a
/// status was invented for it.
#[derive(Debug, Clone)]
pub struct PolicyViolationRecord {
    /// The `PolicyId` of the policy this resource violates. Also the first
    /// segment of the record's key, so one policy's violations are a contiguous
    /// range.
    pub policy_id: String,
    /// The organisation account that owns the violating resource. The second key
    /// segment, so one account's violations under one policy are contiguous too.
    pub account_id: String,
    /// The violating resource's own id (`ComplianceViolator$ResourceId`). The
    /// last key segment, so one resource is one violation of one policy.
    pub resource_id: String,
    /// The violating resource's type (`ComplianceViolator$ResourceType`).
    pub resource_type: Option<String>,
    /// Why the resource is a violator: one of
    /// [`crate::ops::types::VIOLATION_REASON_VALUES`]
    /// (`ComplianceViolator$ViolationReason`).
    pub violation_reason: Option<String>,
    /// Further detail about the violating resource
    /// (`ComplianceViolator$Metadata`).
    pub metadata: Option<HashMap<String, String>>,
    /// What the evaluation found wrong with the resource, as the per-resource
    /// read reports it (`ViolationDetail$ResourceViolations`): one entry per
    /// finding, each naming the finding's own kind through the `ResourceViolation`
    /// member that models it.
    ///
    /// Held here rather than derived, for the reason the inventory exists at all:
    /// a finding of this shape is what an evaluation of the resource PRODUCES,
    /// and this emulator stands in for no evaluator — so the detail is exactly
    /// what was recorded with the violation, and a violation recorded with no
    /// finding of this shape reports none rather than one worded here.
    pub resource_violations: Vec<ResourceViolation>,
    /// The tags the violating resource carried when the violation was recorded
    /// (`ViolationDetail$ResourceTags`). `None` when the evaluation recorded
    /// none: the violating resource belongs to another service, so its tags are
    /// not this store's own tag index ([`ComAmazonawsFmsAwsfms20180101State::tags_by_arn`],
    /// which holds the tags of FIREWALL MANAGER resources).
    pub resource_tags: Option<Vec<Tag>>,
    /// The "brief description for the requested resource"
    /// (`ViolationDetail$ResourceDescription`) the evaluation recorded, or `None`
    /// when it recorded none.
    pub resource_description: Option<String>,
    /// When the evaluation recorded this violation. It is what moves a
    /// compliance status's `LastUpdated` when the evaluation moves — see
    /// [`ComAmazonawsFmsAwsfms20180101State::policy_compliance_status`].
    pub detected_at: SystemTime,
}

impl PolicyViolationRecord {
    /// The key this record is held under in
    /// [`ComAmazonawsFmsAwsfms20180101State::policy_violations`].
    pub fn key(&self) -> String {
        policy_violation_key(&self.policy_id, &self.account_id, &self.resource_id)
    }

    /// This violation as the modelled `ComplianceViolator`, every member read
    /// off the record, so a reader of the inventory reports what was recorded
    /// rather than re-deriving it.
    pub fn violator(&self) -> ComplianceViolator {
        ComplianceViolator {
            resource_id: Some(self.resource_id.clone()),
            violation_reason: self.violation_reason.clone(),
            resource_type: self.resource_type.clone(),
            metadata: self.metadata.clone(),
        }
    }
}

/// The key one recorded violation is held under in
/// [`ComAmazonawsFmsAwsfms20180101State::policy_violations`]: the policy it was
/// recorded under, then the account that owns the violating resource, then the
/// resource's own id.
///
/// The segments are joined by `#`, which none of them can contain (the model's
/// `com.amazonaws.fms#PolicyId` is `^[a-z0-9A-Z-]{36}$`, its `AWSAccountId`
/// `^[0-9]+$` and its `ResourceId` `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`), so
/// [`policy_violation_account_prefix`] is an unambiguous prefix of exactly one
/// account's violations under one policy.
pub fn policy_violation_key(policy_id: &str, account_id: &str, resource_id: &str) -> String {
    format!("{policy_id}#{account_id}#{resource_id}")
}

/// The prefix every key of `policy_id`'s violations in `account_id` starts with
/// — the lower bound of that account's contiguous range in
/// [`ComAmazonawsFmsAwsfms20180101State::policy_violations`]. See
/// [`policy_violation_key`].
pub fn policy_violation_account_prefix(policy_id: &str, account_id: &str) -> String {
    format!("{policy_id}#{account_id}#")
}

/// Per-service state for com-amazonaws-fms-awsfms-20180101. A handler reaches it through [`service_state`].
///
/// `policies` is a `BTreeMap` so a page boundary is a key in a total order:
/// `ListPolicies` can resume from a token without holding a cursor, and a
/// policy added or deleted between pages cannot reshuffle the ones already
/// returned.
#[derive(Debug, Default)]
pub struct ComAmazonawsFmsAwsfms20180101State {
    /// The single Firewall Manager administrator association, if one was made.
    pub admin_account: Option<AdminAccountRecord>,
    /// The Firewall Manager administrator accounts `PutAdminAccount` onboarded,
    /// with the `AdminScope` each of them may manage, keyed by account id.
    ///
    /// A `BTreeMap` for the same reason as [`Self::policies`]: a page boundary
    /// of the administrator enumeration is then a key in a total order rather
    /// than a held cursor.
    ///
    /// The organization's DEFAULT administrator is not in here — it is
    /// [`Self::admin_account`], whose scope is the whole organization — so the
    /// two cannot hold two scopes for one role.
    pub organization_admins: BTreeMap<String, OrganizationAdminRecord>,
    /// Stored policies, keyed by the `PolicyId` this service minted.
    pub policies: BTreeMap<String, PolicyRecord>,
    /// Tags of every Firewall Manager resource, keyed by its ARN: the ARN-keyed
    /// INDEX the tagging operations (`ListTagsForResource`, `TagResource`,
    /// `UntagResource`) work from, since they name a resource by ARN rather than
    /// by the id of its own kind.
    ///
    /// Which copy wins: the TAGS ON THE RECORD of the resource are authoritative
    /// ([`PolicyRecord::tags`], [`ResourceSetRecord::tags`],
    /// [`AppsListRecord::tags`], [`ProtocolsListRecord::tags`]) and this map is
    /// maintained from them by ONE writer,
    /// [`ComAmazonawsFmsAwsfms20180101State::refresh_tag_index`]: every writer
    /// and deleter of a taggable resource calls it with the record's ARN once the
    /// record is written, and it reads the entry's value back off that record, so
    /// the two cannot disagree. A tag write therefore goes through
    /// [`ComAmazonawsFmsAwsfms20180101State::set_tags`], which writes the record
    /// and then refreshes this entry from it; a tag read goes through
    /// [`ComAmazonawsFmsAwsfms20180101State::tags_for_arn`], which reads the
    /// authoritative record rather than this index.
    pub tags_by_arn: HashMap<String, Vec<Tag>>,
    /// Stored resource sets, keyed by the `Base62Id` this service minted. A
    /// `BTreeMap` for the same reason as `policies`: a page boundary is a key in
    /// a total order.
    pub resource_sets: BTreeMap<String, ResourceSetRecord>,
    /// Stored applications lists, keyed by `ListId`.
    pub apps_lists: BTreeMap<String, AppsListRecord>,
    /// Stored protocols lists, keyed by `ListId`.
    pub protocols_lists: BTreeMap<String, ProtocolsListRecord>,
    /// The SNS notification channel each Region of this account has designated,
    /// keyed by the Region the designating request was signed for
    /// ([`request_region`]).
    ///
    /// Keyed by Region because the channel is a REGIONAL configuration and its
    /// three operations model no member that could select one: the Region the
    /// request is signed for is the only thing that says which channel the call
    /// acts on, so a Region that designated nothing must hold nothing — and a
    /// write or a delete in one Region must leave every other Region's channel
    /// alone. A `BTreeMap` for the same reason as [`Self::policies`]: the keys
    /// are in a total order.
    pub notification_channels: BTreeMap<String, NotificationChannelRecord>,
    /// The third-party firewall associations, keyed by the `ThirdPartyFirewall`
    /// enum value. An absent entry is `NOT_EXIST`, not an error.
    pub third_party_firewalls: BTreeMap<String, ThirdPartyFirewallRecord>,
    /// The firewall policies that exist in the third-party firewall vendors'
    /// tenants, keyed by [`third_party_firewall_policy_key`] so ONE vendor's
    /// policies are a contiguous range in id order that a
    /// `ListThirdPartyFirewallFirewallPolicies` page can resume from.
    ///
    /// Like [`Self::shield_attacks`] this is an INVENTORY, not a cache: a
    /// third-party firewall policy is created in the VENDOR's own console or
    /// API — no Firewall Manager operation creates one — so the inventory is
    /// empty unless something put a policy in it.
    /// `ListThirdPartyFirewallFirewallPolicies` reports what is here for the
    /// vendor the request names, and never a policy it made up.
    ///
    /// The organisation's DISCOVERABLE RESOURCES are deliberately not a field
    /// beside this one: Firewall Manager discovers them from the services that
    /// own them, so [`discovered_resources`] reads those services' stores
    /// through the shared engine rather than a copy kept here.
    pub third_party_firewall_policies: BTreeMap<String, ThirdPartyFirewallPolicyRecord>,
    /// The DDoS attacks Shield Advanced recorded against the resources the
    /// stored Shield Advanced policies protect, keyed by
    /// [`shield_attack_key`] so one policy's attacks are a contiguous,
    /// chronologically ordered range a `GetProtectionStatus` page can resume
    /// from.
    ///
    /// This is an INVENTORY, not a cache: Shield Advanced detects attacks and
    /// Firewall Manager reports what it detected, and this emulator stands in
    /// for no attack detector, so the inventory is empty unless something put an
    /// attack in it —
    /// `GetProtectionStatus` reports what is here, filtered by the request's
    /// policy, member account and time period, and never an attack it made up.
    ///
    /// An entry outlives a deletion of its policy that asked for no cleanup of
    /// the policy's resources, and is then unreachable: a `PolicyId` is minted
    /// once and never reused, so no later policy can read another policy's
    /// attacks. A deletion that DID ask for the cleanup takes the policy's
    /// entries with it — see [`Self::delete_policy`].
    pub shield_attacks: BTreeMap<String, ShieldAttackRecord>,
    /// The resources a Firewall Manager policy evaluation found to violate a
    /// stored policy, keyed by [`policy_violation_key`] so one policy's
    /// violations — and, within them, one member account's — are a contiguous
    /// range a compliance page can resume from.
    ///
    /// Like [`Self::shield_attacks`] this is
    /// an INVENTORY, not a cache: see [`PolicyViolationRecord`]. An entry
    /// outlives a deletion of its policy that asked for no cleanup of the
    /// policy's resources and is then unreachable, because a `PolicyId` is
    /// minted once and never reused; a deletion that DID ask for the cleanup
    /// takes the policy's entries with it — see [`Self::delete_policy`].
    pub policy_violations: BTreeMap<String, PolicyViolationRecord>,
}

/// The administrator gate every organisation-scoped Firewall Manager operation
/// runs FIRST lives beside the state it reads: see [`require_administrator`].
impl ComAmazonawsFmsAwsfms20180101State {
    /// Materialise, as of `now`, the disassociation the CLOCK has already made
    /// due: once [`AdminAccountRecord::removal_due`] has passed the administrator
    /// role IS `DELETED` ([`AdminAccountRecord::is_active_at`] already reads it
    /// that way), and this writes that status into the record so a reader reports
    /// it.
    ///
    /// It is the one write a reporting operation (`GetAdminAccount`) may make:
    /// the transition was owed at this instant whether or not anybody called, so
    /// performing it here changes nothing a later caller could not have derived
    /// itself. An operation that only needs to know WHETHER an administrator
    /// exists need not call it at all — the predicate is clock-derived, so
    /// [`require_administrator`] answers the same before and after.
    pub fn settle_admin_account_at(&mut self, now: SystemTime) {
        let Some(admin) = self.admin_account.as_mut() else {
            return;
        };
        if !admin.is_active_at(now) && admin.role_status != ROLE_STATUS_DELETED {
            admin.role_status = ROLE_STATUS_DELETED.to_string();
            admin.removal_due = None;
        }
    }

    /// [`Self::settle_admin_account_at`] at the current instant.
    pub fn settle_admin_account(&mut self) {
        self.settle_admin_account_at(SystemTime::now());
    }

    /// The association that currently makes an account the organisation's
    /// default administrator, after settling what the clock owes. `None` when no
    /// association was ever made and when the last one has been torn down.
    pub fn active_admin_account(&mut self) -> Option<&AdminAccountRecord> {
        let now = SystemTime::now();
        self.settle_admin_account_at(now);
        self.admin_account.as_ref().filter(|a| a.is_active_at(now))
    }

    /// [`Self::active_admin_account`] as of `now`, mutably: the association an
    /// operation that CHANGES the administrator slot works on, so the write site
    /// neither re-derives the active-association predicate nor re-reads the clock
    /// after deciding.
    pub fn active_admin_account_mut_at(
        &mut self,
        now: SystemTime,
    ) -> Option<&mut AdminAccountRecord> {
        self.settle_admin_account_at(now);
        self.admin_account.as_mut().filter(|a| a.is_active_at(now))
    }

    /// Materialise, as of `now`, the onboarding transitions the CLOCK has
    /// already made due on every administrator account `PutAdminAccount`
    /// onboarded (see [`OrganizationAdminRecord::settle`]).
    ///
    /// It is the one write a reporting operation (`GetAdminScope`) may make: the
    /// transition was owed at this instant whether or not anybody called, so
    /// performing it here changes nothing a later caller could not have derived
    /// from [`OrganizationAdminRecord::status_at`] itself.
    pub fn settle_organization_admins_at(&mut self, now: SystemTime) {
        for admin in self.organization_admins.values_mut() {
            admin.settle(now);
        }
    }

    /// The administrator account `PutAdminAccount` onboarded for `account_id`,
    /// after settling what the clock owes it. `None` when that account was never
    /// onboarded as an administrator.
    ///
    /// The DEFAULT administrator is deliberately not resolved here: it is a
    /// different record with a derived scope, and a caller that reports either
    /// of them resolves both explicitly rather than having one stand in for the
    /// other.
    pub fn organization_admin_at(
        &mut self,
        account_id: &str,
        now: SystemTime,
    ) -> Option<&OrganizationAdminRecord> {
        self.settle_organization_admins_at(now);
        self.organization_admins.get(account_id)
    }

    /// The administrator role `account_id` still HOLDS at `now`, mutably: the
    /// record an operation that CHANGES that account's administrator role works
    /// on, so the write site neither re-derives the still-an-administrator
    /// predicate ([`OrganizationAdminRecord::is_administrator_at`]) nor re-reads
    /// the clock after deciding — the counterpart of
    /// [`Self::active_admin_account_mut_at`] for the administrators
    /// `PutAdminAccount` onboards.
    ///
    /// Unlike [`Self::organization_admin_at`] it settles NOTHING: the status it
    /// answers from is clock-derived either way, and a caller that resolves a
    /// role only to refuse the request must leave the store exactly as it was.
    pub fn organization_admin_mut_at(
        &mut self,
        account_id: &str,
        now: SystemTime,
    ) -> Option<&mut OrganizationAdminRecord> {
        self.organization_admins
            .get_mut(account_id)
            .filter(|admin| admin.is_administrator_at(now))
    }

    /// The stored Firewall Manager resource whose ARN is `arn` — whatever KIND of
    /// taggable resource it is — together with the tags it carries, or `None`
    /// when this store holds no resource under that ARN.
    ///
    /// This is the ARN -> resource resolution every tagging operation does before
    /// it reads or writes a tag. A `ResourceArn` is generic — nothing in it tells
    /// the operation which kind of resource it names — so the resolution searches
    /// EVERY kind of resource this store holds that carries tags, and an ARN
    /// naming none of them is the modelled not-found.
    ///
    /// The tags come back WITH the resolution, read off the record that owns them
    /// (the authoritative copy): a resolved resource always has a tag set, an
    /// empty one when it carries no tags, so no caller has to handle a resolved
    /// resource whose tags are missing. Each kind is scanned only until one
    /// matches.
    pub fn tagged_resource_for_arn(&self, arn: &str) -> Option<(TaggedResource, &[Tag])> {
        let in_policies = || {
            self.policies
                .iter()
                .find(|(_, record)| record.policy_arn == arn)
                .map(|(id, record)| {
                    (TaggedResource::Policy(id.clone()), record.tags.as_slice())
                })
        };
        let in_resource_sets = || {
            self.resource_sets
                .iter()
                .find(|(_, record)| record.arn == arn)
                .map(|(id, record)| {
                    (
                        TaggedResource::ResourceSet(id.clone()),
                        record.tags.as_slice(),
                    )
                })
        };
        let in_apps_lists = || {
            self.apps_lists
                .iter()
                .find(|(_, record)| record.list_arn == arn)
                .map(|(id, record)| {
                    (TaggedResource::AppsList(id.clone()), record.tags.as_slice())
                })
        };
        let in_protocols_lists = || {
            self.protocols_lists
                .iter()
                .find(|(_, record)| record.list_arn == arn)
                .map(|(id, record)| {
                    (
                        TaggedResource::ProtocolsList(id.clone()),
                        record.tags.as_slice(),
                    )
                })
        };
        in_policies()
            .or_else(in_resource_sets)
            .or_else(in_apps_lists)
            .or_else(in_protocols_lists)
    }

    /// The tags the resource named by `arn` carries, which is what a tag READER
    /// needs: the resolution of [`Self::tagged_resource_for_arn`], keeping only
    /// the tags. `None` when no stored resource carries that ARN.
    ///
    /// An existing resource that carries no tags answers an EMPTY list: it exists
    /// and holds nothing, which is not the same as nothing being known about it.
    pub fn tags_for_arn(&self, arn: &str) -> Option<Vec<Tag>> {
        self.tagged_resource_for_arn(arn)
            .map(|(_, tags)| tags.to_vec())
    }

    /// Make the ARN-keyed tag index agree with the AUTHORITATIVE record for
    /// `arn`: the entry becomes exactly the tags the stored resource carrying
    /// that ARN holds — of whichever taggable kind it is — and is removed when
    /// this store holds no such resource any more.
    ///
    /// This is the ONE writer of [`Self::tags_by_arn`]: every writer and deleter
    /// of a taggable resource calls it AFTER writing the record, so the rule
    /// "the index entry equals the record's tags, keyed by the record's own ARN"
    /// is stated here and nowhere else. Because it reads the tags through
    /// [`Self::tags_for_arn`] — off the record that owns them — an index entry
    /// that disagrees with the record it mirrors is not expressible, and a fifth
    /// taggable kind needs no new copy of the maintenance step.
    pub fn refresh_tag_index(&mut self, arn: &str) {
        match self.tags_for_arn(arn) {
            // A stored resource: the entry is its own tags.
            Some(tags) => {
                self.tags_by_arn.insert(arn.to_string(), tags);
            }
            // No record, so no entry — the tags of a removed resource do not
            // outlive it.
            None => {
                self.tags_by_arn.remove(arn);
            }
        }
    }

    /// Make `tags` the tag set `resource` carries, for a resource of ANY taggable
    /// kind.
    ///
    /// The record is written first and [`Self::tags_by_arn`] refreshed from it
    /// through [`Self::refresh_tag_index`], so the authoritative copy and the
    /// index can never disagree. Answers whether the resource was still there to
    /// write to.
    pub fn set_tags(&mut self, resource: &TaggedResource, tags: Vec<Tag>) -> bool {
        let arn = match resource {
            TaggedResource::Policy(id) => self.policies.get_mut(id).map(|record| {
                record.tags = tags;
                record.policy_arn.clone()
            }),
            TaggedResource::ResourceSet(id) => self.resource_sets.get_mut(id).map(|record| {
                record.tags = tags;
                record.arn.clone()
            }),
            TaggedResource::AppsList(id) => self.apps_lists.get_mut(id).map(|record| {
                record.tags = tags;
                record.list_arn.clone()
            }),
            TaggedResource::ProtocolsList(id) => {
                self.protocols_lists.get_mut(id).map(|record| {
                    record.tags = tags;
                    record.list_arn.clone()
                })
            }
        };
        match arn {
            Some(arn) => {
                self.refresh_tag_index(&arn);
                true
            }
            None => false,
        }
    }

    /// Every Firewall Manager administrator account this organisation holds,
    /// keyed by account id, as the `AdminAccountSummary` an enumeration reports.
    ///
    /// The organisation's administrators are the two kinds of record this store
    /// keeps: the ONE default administrator `AssociateAdminAccount` associated
    /// ([`AdminAccountRecord`], which "has full administrative scope", so
    /// `DefaultAdmin` is set on it and on no other), and every account
    /// `PutAdminAccount` onboarded ([`OrganizationAdminRecord`]). An account
    /// recorded as both is ONE administrator, and the default-administrator role
    /// is the one reported for it.
    ///
    /// Each summary's `Status` is the `OrganizationStatus` that holds AT `now`,
    /// derived from the CLOCK rather than from whether a writer has
    /// materialised it ([`OrganizationAdminRecord::status_at`] for an onboarded
    /// administrator, and for the default administrator the role status that
    /// holds at `now` read through [`default_admin_organization_status`]), so no
    /// reader of this store can disagree with another about an administrator's
    /// status.
    ///
    /// A `BTreeMap` so the order is total and a page boundary is a KEY rather
    /// than a cursor: an administrator onboarded or removed between two pages
    /// cannot reshuffle the accounts already reported.
    pub fn admin_account_summaries(&self, now: SystemTime) -> BTreeMap<String, AdminAccountSummary> {
        let mut summaries: BTreeMap<String, AdminAccountSummary> = self
            .organization_admins
            .values()
            .map(|admin| {
                (
                    admin.account_id.clone(),
                    AdminAccountSummary {
                        admin_account: Some(admin.account_id.clone()),
                        default_admin: false,
                        status: Some(admin.status_at(now).to_string()),
                    },
                )
            })
            .collect();
        if let Some(admin) = self.admin_account.as_ref() {
            // A tear-down the clock has already completed IS a deleted role,
            // whether or not it has been written yet.
            let role_status = if admin.is_active_at(now) {
                admin.role_status.as_str()
            } else {
                ROLE_STATUS_DELETED
            };
            summaries.insert(
                admin.account_id.clone(),
                AdminAccountSummary {
                    admin_account: Some(admin.account_id.clone()),
                    default_admin: true,
                    status: Some(default_admin_organization_status(role_status).to_string()),
                },
            );
        }
        summaries
    }

    /// The organisation's Firewall Manager administrators OTHER than its
    /// DEFAULT administrator that are still administrators AT `now`, in
    /// ascending account-id order.
    ///
    /// These are the accounts that have to leave before the default
    /// administrator may: `DisassociateAdminAccount` documents that
    /// "disassociation of the default administrator account follows the first
    /// in, last out principle. If you are the default administrator, all
    /// Firewall Manager administrators within the organization must first
    /// disassociate their accounts before you can disassociate your account."
    ///
    /// Read off [`Self::admin_account_summaries`], this store's ONE answer to
    /// which accounts are administrators of the organisation and with which
    /// `OrganizationStatus`, so the precondition and the enumeration cannot
    /// disagree about who is still an administrator. The default administrator
    /// is the summary that carries `DefaultAdmin`, and an account whose status
    /// is [`ORGANIZATION_STATUS_OFFBOARDING_COMPLETE`] "has been removed as an
    /// Firewall Manager administrator" (`AdminAccountSummary$Status`), so it is
    /// no longer one of them.
    ///
    /// It takes `now` for the reason [`AdminAccountRecord::is_active_at`] does:
    /// one request decides from ONE reading of a clock that is not monotonic.
    pub fn other_administrator_accounts(&self, now: SystemTime) -> Vec<String> {
        self.admin_account_summaries(now)
            .into_values()
            .filter(|summary| {
                !summary.default_admin
                    && summary.status.as_deref() != Some(ORGANIZATION_STATUS_OFFBOARDING_COMPLETE)
            })
            .filter_map(|summary| summary.admin_account)
            .collect()
    }

    /// The Firewall Manager administrator accounts that manage the calling
    /// account, in account-id order.
    ///
    /// An organisation has ONE default administrator and its `AdminScope` is the
    /// whole organisation, so while an association exists it manages every
    /// account in that organisation — including the caller. With no association
    /// there is no administrator to enumerate at all, which the caller is told
    /// with the modelled `ResourceNotFoundException` rather than an empty page.
    ///
    /// An association `DisassociateAdminAccount` has torn down
    /// ([`AdminAccountRecord::is_active_at`] is false) manages nothing any more,
    /// so it is left out.
    ///
    /// An administrator `PutAdminAccount` onboarded manages only what its
    /// `AdminScope` says it may, so it is reported here exactly when its
    /// `AccountScope` covers the calling account
    /// ([`admin_scope_manages_account`]) — the store's own answer to "which
    /// administrators manage this account", rather than a second reading of the
    /// scopes it holds.
    ///
    /// It takes `now` for the same reason [`AdminAccountRecord::is_active_at`]
    /// does: one request decides from ONE reading of a clock that is not
    /// monotonic, so two reads inside one handler cannot straddle a settle
    /// boundary.
    pub fn managing_admin_accounts(&self, now: SystemTime) -> Vec<String> {
        let caller = ::engine::emulator_account_id();
        let mut accounts: BTreeSet<String> = self
            .admin_account
            .iter()
            .filter(|admin| admin.is_active_at(now))
            .map(|admin| admin.account_id.clone())
            .collect();
        accounts.extend(
            self.organization_admins
                .values()
                .filter(|admin| {
                    admin.status_at(now) == ORGANIZATION_STATUS_ONBOARDING_COMPLETE
                        && admin_scope_manages_account(&admin.admin_scope, caller)
                })
                .map(|admin| admin.account_id.clone()),
        );
        accounts.into_iter().collect()
    }

    /// The accounts of the administrator's organization this store records, in
    /// ascending account-id order.
    ///
    /// Firewall Manager learns its organization's accounts from Organizations,
    /// which this emulator does not stand in for; what it DOES hold is every
    /// account id its own control plane was told about:
    ///
    /// * the associated administrator account — `AssociateAdminAccount`
    ///   documents that it "must be a member account of the organization in
    ///   Organizations whose resources you want to protect";
    /// * every account id a stored policy names under the
    ///   [`SCOPE_ID_TYPE_ACCOUNT`] key of its `IncludeMap` or `ExcludeMap` —
    ///   `CustomerPolicyScopeMap`'s accounts are the organization's accounts the
    ///   policy applies to or leaves out.
    ///
    /// Values that are not `AWSAccountId`s (the model's
    /// `com.amazonaws.fms#AWSAccountId`: length 1..1024, `^[0-9]+$`) are left
    /// out: a scope map's `CustomerPolicyScopeId` is a free-form string, and an
    /// account list may only carry account ids.
    ///
    /// A `BTreeSet` so the order is total and an account named by two policies
    /// is reported once: a page boundary is then a key, not a cursor.
    ///
    /// An association `DisassociateAdminAccount` has torn down is not an
    /// administrator of the organization any more, so its account is only
    /// recorded here while the association is active AT `now`.
    ///
    /// It takes `now` for the same reason [`AdminAccountRecord::is_active_at`]
    /// does: one request decides from ONE reading of a clock that is not
    /// monotonic, so the roster a request classifies against cannot straddle a
    /// settle boundary the same request's gate decided on the near side of.
    pub fn member_accounts(&self, now: SystemTime) -> BTreeSet<String> {
        let mut accounts = BTreeSet::new();
        if let Some(admin) = self.admin_account.as_ref().filter(|a| a.is_active_at(now)) {
            accounts.insert(admin.account_id.clone());
        }
        // Every account `PutAdminAccount` onboarded as an administrator: that
        // operation documents that the account "must be a member of the
        // organization that was onboarded to Firewall Manager by
        // AssociateAdminAccount", so the store was told about it as a member of
        // the organization.
        accounts.extend(self.organization_admins.keys().cloned());
        for record in self.policies.values() {
            for scope_map in [&record.policy.include_map, &record.policy.exclude_map]
                .into_iter()
                .flatten()
            {
                let Some(scoped) = scope_map.get(SCOPE_ID_TYPE_ACCOUNT) else {
                    continue;
                };
                accounts.extend(
                    scoped
                        .iter()
                        .filter(|id| is_account_id(id))
                        .map(String::clone),
                );
            }
        }
        accounts
    }

    /// The organisation accounts the stored `policy` applies to, in ascending
    /// account-id order.
    ///
    /// `Policy$IncludeMap`'s own documentation is the rule: "If you specify an
    /// `IncludeMap`, Firewall Manager applies the policy to all accounts
    /// specified by the `IncludeMap`, and does not evaluate any `ExcludeMap`
    /// specifications. If you do not specify an `IncludeMap`, then Firewall
    /// Manager applies the policy to all accounts except for those specified by
    /// the `ExcludeMap`."
    ///
    /// The accounts a scope map names are the values under its
    /// [`SCOPE_ID_TYPE_ACCOUNT`] key that ARE account ids ([`is_account_id`]): a
    /// `CustomerPolicyScopeId` is a free-form string, so a value that is not an
    /// `AWSAccountId` names no account — the same reading [`Self::member_accounts`]
    /// applies to the same maps. "All accounts" is the organisation's accounts
    /// this store records, which is exactly that set.
    ///
    /// An `ORG_UNIT` key contributes no accounts: specifying an OU "is the
    /// equivalent of specifying all accounts in the OU and in any of its child
    /// OUs", which is membership Organizations holds and this emulator stands in
    /// for no Organizations — so those accounts are UNKNOWN here rather than
    /// guessed at, and a policy scoped only to OUs applies to no account this
    /// store can name.
    ///
    /// A `BTreeSet` so the order is total and an account named twice is one
    /// account: a page boundary over the scope is then a key, not a cursor.
    ///
    /// It takes `now` because "all accounts" is read through
    /// [`Self::member_accounts`], which is clock-derived: one request decides
    /// from ONE reading of a clock that is not monotonic.
    pub fn policy_scope_accounts(&self, policy: &Policy, now: SystemTime) -> BTreeSet<String> {
        /// The account ids a scope map names, or none when it names no account.
        fn scoped_accounts(
            map: Option<&HashMap<String, Vec<String>>>,
        ) -> BTreeSet<String> {
            map.and_then(|map| map.get(SCOPE_ID_TYPE_ACCOUNT))
                .map(|scoped| {
                    scoped
                        .iter()
                        .filter(|id| is_account_id(id))
                        .map(String::clone)
                        .collect()
                })
                .unwrap_or_default()
        }

        // "If you specify an IncludeMap": a map that names nothing scopes the
        // policy to nothing (the reading `PutPolicy` applies to the same two
        // maps when it refuses a document that scopes with both), so it is no
        // specification and the ExcludeMap reading below applies.
        let included = policy
            .include_map
            .as_ref()
            .is_some_and(|map| map.values().any(|ids| !ids.is_empty()));
        if included {
            return scoped_accounts(policy.include_map.as_ref());
        }
        let excluded = scoped_accounts(policy.exclude_map.as_ref());
        self.member_accounts(now)
            .into_iter()
            .filter(|account| !excluded.contains(account))
            .collect()
    }

    /// Whether the stored `policy` applies to the member account `account`.
    ///
    /// The ONE resolution every operation that reports about a (policy, member
    /// account) pair uses: Firewall Manager evaluates a policy only against the
    /// accounts in the policy's own scope ([`Self::policy_scope_accounts`]), so
    /// an account outside that scope was never evaluated against this policy and
    /// no surface of the policy holds anything about it — which is the modelled
    /// not-found, not an empty report.
    ///
    /// It takes `now` for the reason [`Self::policy_scope_accounts`] does: one
    /// request decides from ONE reading of a clock that is not monotonic, so a
    /// handler cannot resolve a named account against one reading and the page it
    /// serves against another.
    pub fn policy_applies_to_account(
        &self,
        policy: &Policy,
        account: &str,
        now: SystemTime,
    ) -> bool {
        self.policy_scope_accounts(policy, now).contains(account)
    }

    /// The `AdminScope` that narrows what the CALLING account may manage AT
    /// `now`, or `None` when nothing narrows it then.
    ///
    /// An account `PutAdminAccount` onboarded carries an `AdminScope` — "the
    /// resources that the specified Firewall Manager administrator can manage"
    /// — but holding the record is not the same as the scope being IN FORCE:
    /// only an administrator whose `OrganizationStatus` is
    /// [`ORGANIZATION_STATUS_ONBOARDING_COMPLETE`] "can perform actions on the
    /// resources defined in their AdminScope" (`AdminAccountSummary$Status`),
    /// so a record that is still [`ORGANIZATION_STATUS_ONBOARDING`], or on its
    /// way out ([`OrganizationAdminRecord::is_leaving_at`]), governs nothing
    /// yet or nothing any more. Answering `None` then leaves the caller
    /// administering the organization as its default administrator, whose
    /// scope "allows administration of all policy types, accounts,
    /// organizational units, and Regions" ([`default_admin_scope`]) and which
    /// nothing can narrow.
    ///
    /// The ONE place that question is answered, so every reader that derives a
    /// status or a refusal from an administrative scope
    /// ([`Self::policy_status`], [`Self::resource_set_status`]) applies the
    /// same in-force rule [`Self::managing_admin_accounts`] applies to the very
    /// same record, and no two readers of one administrator can disagree about
    /// whether its scope counts.
    ///
    /// It takes `now` for the reason [`OrganizationAdminRecord::status_at`]
    /// does: the status is CLOCK-derived, so one request decides from ONE
    /// reading of a clock that is not monotonic. It settles nothing — the
    /// status it answers from is derived either way, and a reporting read must
    /// leave the store exactly as it was.
    pub fn governing_admin_scope(&self, now: SystemTime) -> Option<&AdminScope> {
        self.organization_admins
            .get(::engine::emulator_account_id())
            .filter(|admin| admin.status_at(now) == ORGANIZATION_STATUS_ONBOARDING_COMPLETE)
            .map(|admin| &admin.admin_scope)
    }

    /// The `CustomerPolicyStatus` of one stored policy: SERVICE-DERIVED, from
    /// the administrative scope that governs the policy rather than from
    /// anything a caller sent.
    ///
    /// `Policy$PolicyStatus` "indicates whether the policy is in or out of an
    /// admin's policy or Region scope": [`POLICY_STATUS_ACTIVE`] while that
    /// scope covers both the policy's type and its Region — "the administrator
    /// can manage and delete the policy" — and
    /// [`POLICY_STATUS_OUT_OF_ADMIN_SCOPE`] otherwise, where "the administrator
    /// can view the policy, but they can't edit or delete" it.
    ///
    /// The governing scope is the CALLING account's own `AdminScope` while that
    /// scope is IN FORCE ([`Self::governing_admin_scope`]). With none in force,
    /// the caller administers the organization as its default administrator,
    /// which "has full administrative scope that allows administration of all
    /// policy types, accounts, organizational units, and Regions"
    /// ([`default_admin_scope`]) — nothing can narrow it, so every policy is in
    /// it. `PutAdminAccount` refuses to configure a scope for the default
    /// administrator's account, so the two cases cannot overlap.
    ///
    /// Derived on every read, so narrowing an administrator's scope moves the
    /// policies it no longer covers out of scope and widening it moves them
    /// back, and no two readers of one policy can report different statuses.
    ///
    /// It takes `now` because the scope in force is clock-derived: one request
    /// decides from ONE reading of a clock that is not monotonic, so the status
    /// a handler reports and the refusal it builds from that status cannot
    /// straddle the instant the administrator's onboarding completes.
    pub fn policy_status(&self, record: &PolicyRecord, now: SystemTime) -> &'static str {
        let Some(scope) = self.governing_admin_scope(now) else {
            return POLICY_STATUS_ACTIVE;
        };
        let security_service_type = record.policy.security_service_policy_data.r#type.as_str();
        if admin_scope_manages_policy_type(scope, security_service_type)
            && admin_scope_manages_region(scope, &record.region)
        {
            POLICY_STATUS_ACTIVE
        } else {
            POLICY_STATUS_OUT_OF_ADMIN_SCOPE
        }
    }

    /// The `ResourceSetStatus` of one stored resource set: SERVICE-DERIVED, from
    /// the administrative scope that governs the resource set rather than from
    /// anything a caller sent.
    ///
    /// `ResourceSet$ResourceSetStatus` "indicates whether the resource set is in
    /// or out of an admin's Region scope": [`RESOURCE_SET_STATUS_ACTIVE`] while
    /// that scope covers the Region the resource set lives in — "the
    /// administrator can manage and delete the resource set" — and
    /// [`RESOURCE_SET_STATUS_OUT_OF_ADMIN_SCOPE`] otherwise, where "the
    /// administrator can view the resource set, but they can't edit or delete
    /// the resource set".
    ///
    /// The REGION SCOPE alone decides it, which is where this differs from
    /// [`Self::policy_status`]: a resource set carries no policy type, and the
    /// member's own documentation names only "an admin's Region scope".
    ///
    /// The governing scope is resolved exactly as a policy's is: the CALLING
    /// account's own `AdminScope` while it is in force
    /// ([`Self::governing_admin_scope`]), and otherwise the full scope of the
    /// organization's default administrator ([`default_admin_scope`]), which
    /// nothing can narrow — so every resource set is in it.
    ///
    /// Derived on every read, so narrowing an administrator's scope moves the
    /// resource sets it no longer covers out of scope and widening it moves them
    /// back, and no two readers of one resource set can report different
    /// statuses.
    ///
    /// It takes `now` for the reason [`Self::policy_status`] does: the scope in
    /// force is clock-derived.
    pub fn resource_set_status(&self, record: &ResourceSetRecord, now: SystemTime) -> &'static str {
        let Some(scope) = self.governing_admin_scope(now) else {
            return RESOURCE_SET_STATUS_ACTIVE;
        };
        if admin_scope_manages_region(scope, &record.region) {
            RESOURCE_SET_STATUS_ACTIVE
        } else {
            RESOURCE_SET_STATUS_OUT_OF_ADMIN_SCOPE
        }
    }

    /// Why the caller may not EDIT or DELETE this stored policy, or `None` when
    /// it may — the modelled refusal that follows from the status
    /// [`Self::policy_status`] derives, built in ONE place so every mutating
    /// reader of a policy refuses on the same rule and with the same wording.
    ///
    /// The two `CustomerPolicyStatus` values say what the administrator governing
    /// a policy may DO with it, not merely what a reader is told:
    /// [`POLICY_STATUS_ACTIVE`] is "the administrator can manage and delete the
    /// policy", while [`POLICY_STATUS_OUT_OF_ADMIN_SCOPE`] is "the administrator
    /// can view the policy, but they can't edit or delete the policy. Existing
    /// policy protections stay in place" (`Policy$PolicyStatus`). So a policy
    /// reported out of the governing administrative scope is readable but
    /// IMMUTABLE: editing its document and removing it are both refused, and the
    /// protections it already applies are left exactly as they are.
    ///
    /// `InvalidOperationException` — "the operation failed because … the
    /// operation wasn't possible" — is the modelled refusal, declared by every
    /// operation that edits or deletes a policy, so the refusal needs no
    /// unmodelled shape. The message names the policy, the administrator whose
    /// scope does not cover it and the status that follows, which is what the
    /// caller needs to widen the scope (`PutAdminAccount`) and retry.
    ///
    /// Judged per CALL, from the derived status rather than from a stored flag,
    /// so widening the administrator's scope makes the policy editable again
    /// without the policy being touched.
    pub fn policy_mutation_refusal(
        &self,
        record: &PolicyRecord,
        now: SystemTime,
    ) -> Option<InvalidOperationException> {
        if self.policy_status(record, now) != POLICY_STATUS_OUT_OF_ADMIN_SCOPE {
            return None;
        }
        Some(out_of_admin_scope_refusal(
            "policy",
            &record.policy_id,
            POLICY_STATUS_OUT_OF_ADMIN_SCOPE,
        ))
    }

    /// Why the caller may not EDIT or DELETE this stored resource set, or `None`
    /// when it may — the resource-set reading of the SAME rule
    /// [`Self::policy_mutation_refusal`] applies to a policy, built from the
    /// status [`Self::resource_set_status`] derives.
    ///
    /// `ResourceSet$ResourceSetStatus` says what the administrator governing a
    /// resource set may DO with it, not merely what a reader is told:
    /// [`RESOURCE_SET_STATUS_ACTIVE`] is "the administrator can manage and delete
    /// the resource set", while [`RESOURCE_SET_STATUS_OUT_OF_ADMIN_SCOPE`] is
    /// "the administrator can view the resource set, but they can't edit or
    /// delete the resource set. Existing protections stay in place". So a
    /// resource set reported out of the governing administrative scope is
    /// readable but IMMUTABLE: rewriting its document, changing which resources
    /// it holds and removing it are all refused, and what it already protects is
    /// left exactly as it is.
    ///
    /// `InvalidOperationException` — "the operation failed because … the
    /// operation wasn't possible" — is the modelled refusal, declared by every
    /// operation that edits or deletes a resource set, so the refusal needs no
    /// unmodelled shape.
    ///
    /// Judged per CALL, from the derived status rather than from a stored flag,
    /// so widening the administrator's scope makes the resource set editable
    /// again without the resource set being touched.
    pub fn resource_set_mutation_refusal(
        &self,
        record: &ResourceSetRecord,
        now: SystemTime,
    ) -> Option<InvalidOperationException> {
        if self.resource_set_status(record, now) != RESOURCE_SET_STATUS_OUT_OF_ADMIN_SCOPE {
            return None;
        }
        Some(out_of_admin_scope_refusal(
            "resource set",
            &record.id,
            RESOURCE_SET_STATUS_OUT_OF_ADMIN_SCOPE,
        ))
    }

    /// The stored policies that USE the resource set `resource_set_id`, in
    /// `PolicyId` order: the ones whose document names that id among
    /// `ResourceSetIds`, "the unique identifiers of the resource sets used by
    /// the policy" (`Policy$ResourceSetIds`).
    ///
    /// Read off the stored document rather than from an index maintained beside
    /// it, so the answer cannot drift from what `GetPolicy` reports.
    pub fn policies_using_resource_set(&self, resource_set_id: &str) -> Vec<&str> {
        self.policies
            .values()
            .filter(|record| {
                record
                    .policy
                    .resource_set_ids
                    .as_ref()
                    .is_some_and(|ids| ids.iter().any(|id| id == resource_set_id))
            })
            .map(|record| record.policy_id.as_str())
            .collect()
    }

    /// Why the caller may not DELETE the resource set `resource_set_id` while
    /// stored policies still USE it, or `None` when none does.
    ///
    /// A policy's `ResourceSetIds` are "the unique identifiers of the resource
    /// sets used by the policy", and a write of a policy document is accepted
    /// only while every id it names resolves to a resource set this
    /// organisation holds — so a resource set a stored policy names is in use by
    /// that policy, and removing it would leave behind a stored document the
    /// service itself would refuse to write. The removal is therefore refused
    /// while any policy uses the set, and nothing is removed: the caller takes
    /// the set out of the policies that use it (or deletes those policies)
    /// first.
    ///
    /// `InvalidOperationException` — "the operation failed because … the
    /// operation wasn't possible" — is the modelled refusal, declared by the
    /// operation that deletes a resource set, so the refusal needs no unmodelled
    /// shape. The message states what makes the removal impossible and names the
    /// set the request asked for and every policy the store holds using it,
    /// which is what the caller needs to clear the reference and retry.
    ///
    /// Judged per CALL from the stored documents, so a set stops being in use
    /// the moment the last policy naming it is updated or deleted.
    pub fn resource_set_in_use_refusal(
        &self,
        resource_set_id: &str,
    ) -> Option<InvalidOperationException> {
        let users = self.policies_using_resource_set(resource_set_id);
        if users.is_empty() {
            return None;
        }
        let named = if users.len() == 1 {
            format!("policy {}", users[0])
        } else {
            format!("policies {}", users.join(", "))
        };
        Some(InvalidOperationException {
            message: Some(format!(
                "The resource set {resource_set_id} is in use by Firewall Manager {named} and \
                 can't be deleted."
            )),
        })
    }

    /// The violations recorded against ONE stored policy in ONE member account,
    /// in ascending resource-id order.
    ///
    /// The ONE reading of the violation inventory both compliance surfaces make
    /// — the per-account summary ([`Self::policy_compliance_status`]) and the
    /// per-resource detail ([`Self::policy_compliance_detail`]) — so the two can
    /// never disagree about what the evaluation of one account found.
    ///
    /// The records are keyed by [`policy_violation_key`], so one account's
    /// violations under one policy are a CONTIGUOUS range: this walks that range
    /// rather than filtering the whole inventory, and the order it yields them in
    /// is the order of the last key segment, the violating resource's own id.
    pub fn account_violations<'store>(
        &'store self,
        policy_id: &str,
        account_id: &str,
    ) -> impl Iterator<Item = &'store PolicyViolationRecord> + 'store {
        let prefix = policy_violation_account_prefix(policy_id, account_id);
        self.policy_violations
            .range(prefix.clone()..)
            .take_while(move |(key, _)| key.starts_with(&prefix))
            .map(|(_, violation)| violation)
    }

    /// The `PolicyComplianceStatus` of ONE member account under ONE stored
    /// policy: the per-account summary `ListComplianceStatus` reports.
    ///
    /// Every member is read off the store. The evaluation is the violations
    /// recorded against THIS policy in THIS account
    /// ([`Self::account_violations`]): `ViolatorCount` is how many there are
    /// ("the number of resources that are noncompliant with the specified
    /// policy"), the account is [`POLICY_COMPLIANCE_NON_COMPLIANT`] exactly while
    /// at least one is recorded and [`POLICY_COMPLIANCE_COMPLIANT`] otherwise,
    /// and `EvaluationLimitExceeded` is set exactly when more than
    /// [`EVALUATION_VIOLATOR_LIMIT`] of them are.
    ///
    /// `LastUpdated` is "the timestamp of the last update to the
    /// `EvaluationResult` objects", so it is the latest instant anything that
    /// evaluation reads changed: the policy's own last update, or the most
    /// recent violation recorded against this account when that is later. It is
    /// therefore stable across reads and moves only when the state behind it
    /// does.
    ///
    /// `IssueInfoMap` is left out: it carries "details about problems with
    /// dependent services, such as WAF or Config, and the error message received
    /// that indicates the problem with the service", and this store records no
    /// such problem — an empty map would claim a dependent service was checked.
    pub fn policy_compliance_status(
        &self,
        policy: &PolicyRecord,
        account_id: &str,
    ) -> PolicyComplianceStatus {
        let mut violator_count: i64 = 0;
        let mut last_updated = policy.updated_at;
        for violation in self.account_violations(&policy.policy_id, account_id) {
            violator_count += 1;
            last_updated = last_updated.max(violation.detected_at);
        }
        let compliance_status = if violator_count == 0 {
            POLICY_COMPLIANCE_COMPLIANT
        } else {
            POLICY_COMPLIANCE_NON_COMPLIANT
        };
        PolicyComplianceStatus {
            // "The Amazon Web Services account that owns the policy"
            // (`PolicyComplianceStatus$PolicyOwner`): this emulator presents one
            // account, and it is the account every stored policy's ARN names.
            policy_owner: Some(::engine::emulator_account_id().to_string()),
            policy_id: Some(policy.policy_id.clone()),
            policy_name: Some(policy.policy.policy_name.clone()),
            member_account: Some(account_id.to_string()),
            evaluation_results: Some(vec![EvaluationResult {
                compliance_status: Some(compliance_status.to_string()),
                violator_count,
                evaluation_limit_exceeded: violator_count > EVALUATION_VIOLATOR_LIMIT,
            }]),
            last_updated: Some(::aws_smithy_types::DateTime::from(last_updated)),
            issue_info_map: None,
        }
    }

    /// The `PolicyComplianceDetail` of ONE member account under ONE stored
    /// policy: the per-RESOURCE detail `GetComplianceDetail` reports, where
    /// [`Self::policy_compliance_status`] reports the same evaluation counted.
    ///
    /// The evaluation is the one reading both surfaces make
    /// ([`Self::account_violations`]) — the violations recorded against THIS
    /// policy in THIS account — so the detail of an account and its summary can
    /// never disagree: the detail names exactly the resources the summary
    /// counted, in the inventory's own ascending resource-id order, each
    /// projected by [`PolicyViolationRecord::violator`].
    ///
    /// The shape's own documentation is the rule for how much of that evaluation
    /// it reports: "A maximum of 100 entries are displayed. If more than 100
    /// resources are noncompliant, `EvaluationLimitExceeded` is set to `True`".
    /// So `Violators` carries at most [`EVALUATION_VIOLATOR_LIMIT`] of them and
    /// `EvaluationLimitExceeded` is set exactly when MORE than that many are
    /// recorded — the same threshold, read off the same violations, as the
    /// summary's `EvaluationResult$EvaluationLimitExceeded`. An account with
    /// nothing recorded against it has an empty violator list, because no
    /// violating resource of its is known.
    ///
    /// `IssueInfoMap` is left out for the reason it is on the summary. `ExpiredAt`
    /// is left out too: it is "a timestamp that indicates when the returned
    /// information should be considered out of date", so it reports how long this
    /// evaluation stays valid, and nothing in this store holds that — the
    /// violation inventory records when a violation was DETECTED, never when the
    /// finding lapses, and Firewall Manager documents no validity window to
    /// derive one from. A timestamp invented here would claim a freshness
    /// guarantee no recorded state backs.
    pub fn policy_compliance_detail(
        &self,
        policy: &PolicyRecord,
        account_id: &str,
    ) -> PolicyComplianceDetail {
        let limit = usize::try_from(EVALUATION_VIOLATOR_LIMIT).unwrap_or(usize::MAX);
        // ONE walk of the account's range decides both members: every recorded
        // violation is counted, so the flag is about what the evaluation FOUND,
        // while only the first `limit` of them are reported, which is what the
        // flag then tells the caller it is missing.
        let mut recorded: usize = 0;
        let mut violators: Vec<ComplianceViolator> = Vec::new();
        for violation in self.account_violations(&policy.policy_id, account_id) {
            recorded += 1;
            if violators.len() < limit {
                violators.push(violation.violator());
            }
        }
        PolicyComplianceDetail {
            // "The Amazon Web Services account that created the Firewall Manager
            // policy" (`PolicyComplianceDetail$PolicyOwner`): this emulator
            // presents one account, and it is the account every stored policy's
            // ARN names — the same reading the summary makes.
            policy_owner: Some(::engine::emulator_account_id().to_string()),
            policy_id: Some(policy.policy_id.clone()),
            member_account: Some(account_id.to_string()),
            violators: Some(violators),
            evaluation_limit_exceeded: recorded > limit,
            expired_at: None,
            issue_info_map: None,
        }
    }

    /// The `ViolationDetail` of ONE violating resource under ONE stored policy
    /// in ONE member account: the per-resource read `GetViolationDetails`
    /// reports, where [`Self::policy_compliance_detail`] reports the same
    /// evaluation's resources NAMED and [`Self::policy_compliance_status`]
    /// reports them counted.
    ///
    /// It is the same violation inventory all three read, so no two of them can
    /// disagree about what the evaluation of one account found: a resource this
    /// answers for is one the compliance detail names as a violator, and every
    /// member of the answer is read off that recorded violation.
    ///
    /// `None` — the caller's not-found — when the inventory records no violation
    /// of this policy by this resource in this account, and also when it records
    /// one of a DIFFERENT resource type than the request names: the request
    /// identifies the resource by its type as well as by its id ("The resource
    /// type. This is in the format shown in the Amazon Web Services Resource
    /// Types Reference"), so a recorded violation of another type is not the
    /// resource that was asked about, and a violation recorded with no resource
    /// type at all names no type to match.
    pub fn resource_violation_detail(
        &self,
        policy_id: &str,
        account_id: &str,
        resource_id: &str,
        resource_type: &str,
    ) -> Option<ViolationDetail> {
        let violation = self
            .policy_violations
            .get(&policy_violation_key(policy_id, account_id, resource_id))?;
        // Read off the RECORD, not off the request: the two are equal here by
        // the filter, and the record is what the evaluation found.
        let recorded_type = violation
            .resource_type
            .clone()
            .filter(|recorded| recorded == resource_type)?;
        Some(ViolationDetail {
            policy_id: violation.policy_id.clone(),
            member_account: violation.account_id.clone(),
            resource_id: violation.resource_id.clone(),
            resource_type: recorded_type,
            resource_violations: violation.resource_violations.clone(),
            resource_tags: violation.resource_tags.clone(),
            resource_description: violation.resource_description.clone(),
        })
    }

    /// The stored applications list `list_id` names IN THE NAMESPACE
    /// `default_list` selects — Firewall Manager MANAGED (default) lists when it
    /// is true, customer-created ones when it is false — or `None` when that
    /// namespace holds no list under that id.
    ///
    /// The two namespaces are separate: `GetAppsList` takes a `DefaultList`
    /// member and `ListAppsLists` a `DefaultLists` member precisely to choose
    /// between them, so an id is only ever resolved WITHIN the namespace the
    /// request addresses and a list of the other kind is simply not there.
    pub fn apps_list_in_namespace(
        &self,
        list_id: &str,
        default_list: bool,
    ) -> Option<&AppsListRecord> {
        self.apps_lists
            .get(list_id)
            .filter(|record| record.default_list == default_list)
    }

    /// [`Self::apps_list_in_namespace`] for protocols lists, whose namespaces
    /// `GetProtocolsList`'s `DefaultList` and `ListProtocolsLists`'
    /// `DefaultLists` split the same way.
    pub fn protocols_list_in_namespace(
        &self,
        list_id: &str,
        default_list: bool,
    ) -> Option<&ProtocolsListRecord> {
        self.protocols_lists
            .get(list_id)
            .filter(|record| record.default_list == default_list)
    }

    /// Remove the CUSTOMER applications list `list_id` names, dropping its
    /// entry in the ARN-keyed tag index with it, and answer whether there was
    /// such a list — `false` leaves the store untouched.
    ///
    /// A Firewall Manager MANAGED (default) list is AWS's own and is not in the
    /// namespace a customer delete addresses (that operation's input models no
    /// `DefaultList` selector at all), so an id naming one removes nothing and
    /// the caller is told the resource was not found — the same resolution
    /// `PutAppsList` / `PutProtocolsList` do before they write.
    pub fn delete_customer_apps_list(&mut self, list_id: &str) -> bool {
        let Some(arn) = self
            .apps_list_in_namespace(list_id, false)
            .map(|record| record.list_arn.clone())
        else {
            return false;
        };
        self.apps_lists.remove(list_id);
        // The record is gone, so the single index writer drops its entry.
        self.refresh_tag_index(&arn);
        true
    }

    /// [`Self::delete_customer_apps_list`] for protocols lists: the same
    /// namespace rule, so the two deleters cannot disagree about what a
    /// customer may remove.
    pub fn delete_customer_protocols_list(&mut self, list_id: &str) -> bool {
        let Some(arn) = self
            .protocols_list_in_namespace(list_id, false)
            .map(|record| record.list_arn.clone())
        else {
            return false;
        };
        self.protocols_lists.remove(list_id);
        // The record is gone, so the single index writer drops its entry.
        self.refresh_tag_index(&arn);
        true
    }

    /// Remove the policy `policy_id` names, and the records this store holds on
    /// that policy's BEHALF when `delete_all_resources` asks for the cleanup.
    /// Answers the removed [`PolicyRecord`], or `None` when no policy carries
    /// that id — in which case nothing is removed at all.
    ///
    /// Two different things go in a policy delete, and the request's cleanup
    /// selector decides only the second:
    ///
    /// * the POLICY itself — its record and its entry in the ARN-keyed tag index
    ///   — always, since that is what the delete is for;
    /// * the objects Firewall Manager holds BECAUSE of the policy, which the
    ///   cleanup selector governs: a delete that asks for the cleanup takes them
    ///   with the policy, and a delete that does not leaves them in place. Here
    ///   those objects are what the policy's protections put in this store: the
    ///   violations an evaluation of this policy recorded
    ///   ([`Self::policy_violations`]) and the attacks recorded against the
    ///   resources this policy protects ([`Self::shield_attacks`]). Each is
    ///   keyed under exactly one `PolicyId`, so cleaning up one policy can never
    ///   reach another policy's records — which is also why the cleanup is total
    ///   rather than conditional on some other policy still covering them.
    ///
    /// Each record names the policy it belongs to, so the selection is made from
    /// the RECORD rather than from the shape of its key: a change to either key
    /// format cannot silently widen or narrow what a cleanup removes.
    pub fn delete_policy(
        &mut self,
        policy_id: &str,
        delete_all_resources: bool,
    ) -> Option<PolicyRecord> {
        let removed = self.policies.remove(policy_id)?;
        // The record is gone, so the single index writer drops its entry.
        self.refresh_tag_index(&removed.policy_arn);
        if delete_all_resources {
            self.policy_violations
                .retain(|_, violation| violation.policy_id != policy_id);
            self.shield_attacks
                .retain(|_, attack| attack.policy_id != policy_id);
        }
        Some(removed)
    }
}

/// The highest version number a list's version history can key an archived
/// array under: the key of `PreviousAppsList` and of `PreviousProtocolsList` is
/// `com.amazonaws.fms#PreviousListVersion`, whose pattern is `^\d{1,2}$`, so a
/// version number is at most two digits. A list updated more times than this
/// keeps the 99 versions the map can name and archives no further ones, rather
/// than emitting a key the model refuses.
pub const MAX_PREVIOUS_LIST_VERSION: usize = 99;

/// The version history a list write leaves behind: everything `history` already
/// archives, plus `superseded` — the array the write replaces — under the
/// version number that array held.
///
/// `PreviousAppsList` and `PreviousProtocolsList` are each "a map of previous
/// version numbers to their corresponding … arrays": the archive already holds
/// every version before the stored one, so the version being superseded is one
/// past what the archive names, the create being version 1. It is archived only
/// while that number still fits [`MAX_PREVIOUS_LIST_VERSION`].
///
/// Generic over the archived element, so the applications-list and the
/// protocols-list writer share ONE copy of the version-and-cap rule and cannot
/// disagree about either half of it.
pub fn archive_superseded_list_version<T: Clone>(
    history: Option<&HashMap<String, Vec<T>>>,
    superseded: &[T],
) -> HashMap<String, Vec<T>> {
    let mut history = history.cloned().unwrap_or_default();
    let superseded_version = history.len() + 1;
    if superseded_version <= MAX_PREVIOUS_LIST_VERSION {
        history.insert(superseded_version.to_string(), superseded.to_vec());
    }
    history
}

/// Merge `added` into `existing` the way a tag write does: a key already present
/// takes the new value in place, a key that is new is appended.
///
/// Tags are a key:value MAP (`com.amazonaws.fms#Tag` is one key and one value),
/// so a resource can never hold two values for one key; and "add one or more
/// tags" leaves the keys the request did not mention alone.
pub fn merge_tags(existing: &mut Vec<Tag>, added: &[Tag]) {
    for tag in added {
        match existing.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value.clone(),
            None => existing.push(tag.clone()),
        }
    }
}

/// The first tag key `tag_list` carries more than once, if any.
///
/// Keys are case-SENSITIVE (`com.amazonaws.fms#Tag$Key`: "Tag keys are
/// case-sensitive"), so `env` and `Env` are two keys, not one repeated.
fn first_duplicate_tag_key(tag_list: &[Tag]) -> Option<&str> {
    tag_list
        .iter()
        .enumerate()
        .find(|(index, tag)| {
            tag_list[..*index]
                .iter()
                .any(|earlier| earlier.key == tag.key)
        })
        .map(|(_, tag)| tag.key.as_str())
}

/// Why `tag_list` cannot be written as a tag set, or `None` when every key in it
/// is distinct.
///
/// Tags are a key:value MAP (`com.amazonaws.fms#Tag` is one key and one value),
/// so a resource can never hold two values for one key: a `TagList` naming one
/// key twice has no single outcome and is refused rather than silently resolved.
/// The rule is about the `TagList` a request carries, not about one operation,
/// so EVERY operation that writes one — the tagging operation and each resource
/// writer whose input carries a `TagList` — refuses it here, with the modelled
/// `InvalidInputException` and this one wording.
pub fn duplicate_tag_key_message(tag_list: &[Tag]) -> Option<String> {
    first_duplicate_tag_key(tag_list)
        .map(|duplicate| format!("TagList contains the tag key {duplicate} more than once"))
}

/// The reason `arn` is not an Amazon Resource Name at all, or `None` when it is
/// shaped like one.
///
/// Firewall Manager checks the FORM of a `ResourceArn` before it looks anything
/// up, and the modelled `ResourceArn` pattern cannot express it (that pattern
/// accepts any string of letters, digits and punctuation), so this check lives
/// here and answers a 400 `ValidationException` — a malformed ARN is a caller
/// programming error, distinct from an ARN that is well formed but names
/// nothing.
///
/// The form itself is NOT restated here: it is the one [`parse_arn_form`]
/// applies, so a string these operations call an ARN and a string the
/// resource-set classifier calls an ARN ([`parse_arn`]) are the same string, and
/// this function only words the defect that single parse reports.
pub fn arn_format_error(arn: &str) -> Option<String> {
    parse_arn_form(arn).err().map(|defect| defect.message(arn))
}

/// Whether `value` is an `AWSAccountId` as this service's model declares it
/// (`com.amazonaws.fms#AWSAccountId`: length 1..1024, pattern `^[0-9]+$`).
pub fn is_account_id(value: &str) -> bool {
    (1..=1024).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_digit())
}

/// The `Message` Firewall Manager puts on the `ResourceNotFoundException` of a
/// lookup that NAMES what it looked for, whatever that was.
///
/// The model declares the member but no literal, and the recordings of the live
/// service carry this one string on five different operations, across three
/// fixtures and across two kinds of lookup key (a policy UUID and a resource
/// ARN) — never the identifier that was looked up. So the text is a service
/// constant, not a per-call detail, and this is the single place it is written.
///
/// It is NOT the text of the one not-found this service answers for the
/// caller's own administrator association; that error carries no message at all
/// ([`administrator_association_not_found`]).
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The referenced item does not exist.";

/// The modelled `ResourceNotFoundException` for a resource this service resolved
/// and could not find: the identifier, ARN or name that was looked up never
/// reaches the wire.
///
/// Every not-found site that RESOLVED something goes through here, so no handler
/// can word its own not-found text and no two of them can disagree. Which
/// resource was missing is the caller's own request — it is not restated in the
/// error, exactly as the live service does not restate it.
pub fn resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(RESOURCE_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The modelled `ResourceNotFoundException` reporting that the caller's account
/// has no Firewall Manager administrator association — the account's single
/// administrator slot, which no request names — carrying NO `Message` member at
/// all.
///
/// The live service answers this one with an empty message body: a client reading
/// the exception's message off it reads nothing, which is how the "Firewall
/// Manager onboarding never happened" state is told apart from the not-found of
/// a resource that was looked up by key ([`resource_not_found`], whose recorded
/// text belongs to those keyed lookups). The slot is not a resource the request
/// identifies, so there is nothing for a message to restate.
///
/// Separate from [`resource_not_found`] rather than a parameter of it, so the
/// keyless administrator-slot miss cannot pick up the keyed-lookup text by
/// default, and every site reporting that slot empty answers identically.
pub fn administrator_association_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException { message: None }
}

/// The modelled refusal of an edit or a delete of a resource that the
/// administrative scope governing it does not cover, for a resource of the kind
/// `resource` ("policy", "resource set") carrying the identifier `identifier`
/// and reported with the out-of-scope `status` of its own modelled catalog.
///
/// `OUT_OF_ADMIN_SCOPE` is one rule for every kind of Firewall Manager resource
/// whose status derives from an administrative scope: "the administrator can
/// view the \<resource\>, but they can't edit or delete the \<resource\>". So
/// the refusal is worded ONCE here and taken by every status-aware mutation
/// ([`ComAmazonawsFmsAwsfms20180101State::policy_mutation_refusal`],
/// [`ComAmazonawsFmsAwsfms20180101State::resource_set_mutation_refusal`]), which
/// is what stops two kinds of resource refusing the same thing differently.
///
/// The message names the resource, the administrator whose scope does not cover
/// it and the status that follows, which is what the caller needs to widen the
/// scope (`PutAdminAccount`) and retry.
fn out_of_admin_scope_refusal(
    resource: &str,
    identifier: &str,
    status: &str,
) -> InvalidOperationException {
    InvalidOperationException {
        message: Some(format!(
            "{resource} {identifier} is {status} for Firewall Manager administrator {}: the \
             administrator can view the {resource}, but they can't edit or delete the {resource}",
            ::engine::emulator_account_id()
        )),
    }
}

/// The Firewall Manager administrator the organization has associated, or the
/// service's authorization refusal for a caller that is not entitled to an
/// organization-scoped call.
///
/// Firewall Manager answers an organization-scoped request only for a Firewall
/// Manager administrator or the organization's management account
/// (`ListMemberAccounts`: "Either an Firewall Manager administrator or the
/// organization's management account can make this request"). This emulator
/// presents ONE account, [`::engine::emulator_account_id`], and it is the
/// account that associates the administrator — so what decides the gate is
/// whether an administrator association exists at all. With none, the caller is
/// neither of the two, and the live service refuses the call with the error code
/// `AccessDeniedException`, which this service's model declares for no
/// operation (hence [`crate::scaffolded::Scaffolded::access_denied`], HTTP 400).
///
/// It answers through [`AdminAccountRecord::is_active_at`], so an association
/// `DisassociateAdminAccount` has torn down is no administrator: the gate closes
/// again once the tear-down the clock owed has completed, whether or not any
/// operation has materialised the `DELETED` status yet. It therefore takes the
/// store immutably and writes nothing.
///
/// The refusal's message is BUILT FROM THE REQUEST — it names the calling
/// account and the Region the request was signed for, the two things that decide
/// which administrator association was looked for — so `region` is the caller's
/// Region ([`request_region`]), never a constant.
///
/// Generic over the operation's modeled error enum, so every administrator-gated
/// handler shares this one gate.
///
/// Called BEFORE any identifier is looked up and before any payload is validated,
/// so a nonexistent id, a malformed ARN and a well-formed request all get the SAME
/// refusal: an authorization decision that leaked whether the resource exists would
/// tell an unauthorized caller what the organization holds. A refused call therefore
/// also writes nothing.
///
/// Which operations take this gate is decided PER OPERATION, by what each one
/// answers, never by the resource it names: an operation whose recorded answer to
/// a caller with no administrator association is the outcome of its own lookup is
/// not gated here, and reaches the store directly (`GetPolicy` and `DeletePolicy`
/// are that case — `AccessDeniedException` is in neither one's modeled error set,
/// nor in this service's common errors).
///
/// The refusal [`require_administrator`] states when the organization has no
/// Firewall Manager administrator: HTTP 400 `AccessDeniedException`, naming the
/// calling account and the Region the request was signed for.
///
/// Separate from the gate so an operation that resolves the association itself —
/// one that must hold it MUTABLY to change it, and so cannot take the gate's
/// shared borrow — states the SAME refusal instead of wording its own.
pub fn no_administrator_refusal<E>(region: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::access_denied(format!(
        "No default admin could be found for account {} in Region {region}",
        ::engine::emulator_account_id()
    ))
}

/// The association that authorizes an organization-scoped call AS OF `now`: the
/// administrator that is active at that instant, or [`no_administrator_refusal`]
/// when the organization holds none then.
///
/// It takes `now` for the same reason [`AdminAccountRecord::is_active_at`] does:
/// the gate and everything the request answers from decide from ONE reading of a
/// clock that is not monotonic, so a request cannot be authorized at one instant
/// and answered — or written — from another.
///
/// It is taken only by the operations whose recorded answer to a caller with no
/// administrator association IS this refusal. The resource-level tagging
/// operations are not among them: they name their target by the model's generic
/// `ResourceArn` and nothing else, declare no authorization error in their error
/// unions, and are answered from that ARN alone — its form, then its resolution
/// — so they do not call this.
pub fn require_administrator_at<'store, E>(
    store: &'store ComAmazonawsFmsAwsfms20180101State,
    region: &str,
    now: SystemTime,
) -> Result<&'store AdminAccountRecord, crate::scaffolded::Scaffolded<E>> {
    store
        .admin_account
        .as_ref()
        .filter(|admin| admin.is_active_at(now))
        .ok_or_else(|| no_administrator_refusal(region))
}

/// [`require_administrator_at`] at the current instant: the gate for a handler
/// that makes no OTHER clock-derived decision, so this reading is the whole
/// request's one reading.
///
/// A handler that already holds a reading — because it settles what the clock
/// owes, projects a status as of an instant, or writes a due instant — calls
/// [`require_administrator_at`] with THAT reading instead, the same way
/// [`ComAmazonawsFmsAwsfms20180101State::active_admin_account_mut_at`] exists
/// beside [`ComAmazonawsFmsAwsfms20180101State::active_admin_account`]: a clock
/// that is not monotonic must not be able to authorize a request at one instant
/// and answer it from another.
pub fn require_administrator<'store, E>(
    store: &'store ComAmazonawsFmsAwsfms20180101State,
    region: &str,
) -> Result<&'store AdminAccountRecord, crate::scaffolded::Scaffolded<E>> {
    require_administrator_at(store, region, SystemTime::now())
}

/// The `AdminScope` of the organization's DEFAULT administrator: everything.
///
/// Derived rather than stored, because it follows from the role: the default
/// administrator "has full administrative scope that allows administration of
/// all policy types, accounts, organizational units, and Regions"
/// (`AssociateAdminAccount`), which is exactly every "all … enabled" member of
/// `AdminScope`'s four scopes set. Nothing can narrow it, so there is no stored
/// copy that could drift from the role.
pub fn default_admin_scope() -> AdminScope {
    AdminScope {
        account_scope: Some(AccountScope {
            accounts: None,
            all_accounts_enabled: true,
            exclude_specified_accounts: false,
        }),
        organizational_unit_scope: Some(OrganizationalUnitScope {
            organizational_units: None,
            all_organizational_units_enabled: true,
            exclude_specified_organizational_units: false,
        }),
        region_scope: Some(RegionScope {
            regions: None,
            all_regions_enabled: true,
        }),
        policy_type_scope: Some(PolicyTypeScope {
            policy_types: None,
            all_policy_types_enabled: true,
        }),
    }
}

/// The `OrganizationStatus` of the DEFAULT administrator association, from the
/// `AccountRoleStatus` of its role.
///
/// The two enums describe the same association from two sides — the role's
/// lifecycle (`AccountRoleStatus`) and the account's onboarding as an
/// administrator (`OrganizationStatus`) — so the second follows from the first:
/// a role that is being created is an account that is `ONBOARDING`, a `READY`
/// role is an onboarded administrator, and a role being or already deleted is
/// an administrator being or already removed.
pub fn default_admin_organization_status(role_status: &str) -> &'static str {
    match role_status {
        ROLE_STATUS_READY => ORGANIZATION_STATUS_ONBOARDING_COMPLETE,
        ROLE_STATUS_DELETED => ORGANIZATION_STATUS_OFFBOARDING_COMPLETE,
        ROLE_STATUS_PENDING_DELETION | ROLE_STATUS_DELETING => ORGANIZATION_STATUS_OFFBOARDING,
        _ => ORGANIZATION_STATUS_ONBOARDING,
    }
}

/// Whether an administrator holding `scope` may apply policies to
/// `account_id`.
///
/// This is `AccountScope`'s own documented reading, in one place so no reader
/// re-derives it: `AllAccountsEnabled` covers every account in the
/// organization; otherwise the listed `Accounts` are the accounts covered, or —
/// when `ExcludeSpecifiedAccounts` is set — the accounts NOT covered. An
/// administrator whose scope configures no `AccountScope` at all has been
/// granted no account, so it covers none.
pub fn admin_scope_manages_account(scope: &AdminScope, account_id: &str) -> bool {
    let Some(account_scope) = scope.account_scope.as_ref() else {
        return false;
    };
    if account_scope.all_accounts_enabled {
        return true;
    }
    let listed = account_scope
        .accounts
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|listed| listed == account_id);
    if account_scope.exclude_specified_accounts {
        !listed
    } else {
        listed
    }
}

/// The `SecurityServiceType` of a THIRD-PARTY firewall policy: the policy type
/// `PolicyTypeScope$AllPolicyTypesEnabled` excepts, because "third-party policy
/// types can only be managed by the Firewall Manager default administrator".
/// One of [`crate::ops::types::SECURITY_SERVICE_TYPE_VALUES`].
pub const SECURITY_SERVICE_THIRD_PARTY_FIREWALL: &str = "THIRD_PARTY_FIREWALL";

/// Whether an administrator holding `scope` may manage a policy whose
/// `SecurityServicePolicyData.Type` is `security_service_type`.
///
/// `PolicyTypeScope`'s own documented reading, beside
/// [`admin_scope_manages_account`] so no reader re-derives it:
/// `AllPolicyTypesEnabled` "allows the specified Firewall Manager administrator
/// to manage all Firewall Manager policy types, except for third-party policy
/// types" ([`SECURITY_SERVICE_THIRD_PARTY_FIREWALL`], which "can only be
/// managed by the Firewall Manager default administrator"); otherwise the
/// listed `PolicyTypes` are "the list of policy types that the specified
/// Firewall Manager administrator can manage". A scope that configures no
/// `PolicyTypeScope` at all has been granted no policy type, so it manages
/// none.
pub fn admin_scope_manages_policy_type(scope: &AdminScope, security_service_type: &str) -> bool {
    let Some(policy_type_scope) = scope.policy_type_scope.as_ref() else {
        return false;
    };
    if policy_type_scope.all_policy_types_enabled {
        return security_service_type != SECURITY_SERVICE_THIRD_PARTY_FIREWALL;
    }
    policy_type_scope
        .policy_types
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|listed| listed == security_service_type)
}

/// Whether an administrator holding `scope` may act in `region`.
///
/// `RegionScope`'s own documented reading: `AllRegionsEnabled` "allows the
/// specified Firewall Manager administrator to manage all Amazon Web Services
/// Regions"; otherwise the listed `Regions` are "the Amazon Web Services
/// Regions that the specified Firewall Manager administrator can perform
/// actions in". A scope that configures no `RegionScope` at all has been
/// granted no Region, so it covers none.
pub fn admin_scope_manages_region(scope: &AdminScope, region: &str) -> bool {
    let Some(region_scope) = scope.region_scope.as_ref() else {
        return false;
    };
    region_scope.all_regions_enabled
        || region_scope
            .regions
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|listed| listed == region)
}

/// Why `scope` is not an administrative scope this service can grant, or `None`
/// when it is — the service-level validation of an `AdminScope` a caller sends,
/// built from the request so the refusal names the member and the constraint it
/// broke.
///
/// Each rule is one the model's own documentation states:
///
/// * "You can either enable management of all accounts through this operation,
///   or you can specify a list of accounts to manage in
///   `AccountScope$Accounts`. You cannot specify both."
///   (`AccountScope$AllAccountsEnabled`), and the same sentence for excluding
///   accounts (`AccountScope$ExcludeSpecifiedAccounts`);
/// * the identical pair for organizational units
///   (`OrganizationalUnitScope$AllOrganizationalUnitsEnabled` /
///   `$ExcludeSpecifiedOrganizationalUnits`);
/// * "Third-party policy types can only be managed by the Firewall Manager
///   default administrator" (`PolicyTypeScope$AllPolicyTypesEnabled`), so a
///   delegated administrator's `PolicyTypes` may not name one;
/// * an `AccountScope$Accounts` entry must be an `AWSAccountId` (the model's
///   `com.amazonaws.fms#AWSAccountId`: length 1..1024, `^[0-9]+$`).
///
/// It is a free function, not a method on a handler, so every writer of an
/// administrative scope validates it identically.
pub fn admin_scope_error(scope: &AdminScope) -> Option<String> {
    if let Some(account_scope) = scope.account_scope.as_ref() {
        let listed = account_scope.accounts.as_deref().unwrap_or_default();
        if account_scope.all_accounts_enabled && !listed.is_empty() {
            return Some(
                "AccountScope$AllAccountsEnabled and AccountScope$Accounts cannot both be \
                 specified: either enable management of all accounts or list the accounts to \
                 manage"
                    .to_string(),
            );
        }
        if account_scope.all_accounts_enabled && account_scope.exclude_specified_accounts {
            return Some(
                "AccountScope$AllAccountsEnabled and AccountScope$ExcludeSpecifiedAccounts \
                 cannot both be specified: either enable management of all accounts or exclude \
                 the accounts in AccountScope$Accounts"
                    .to_string(),
            );
        }
        if let Some(invalid) = listed.iter().find(|id| !is_account_id(id)) {
            return Some(format!(
                "AccountScope$Accounts may only name Amazon Web Services account ids \
                 (^[0-9]+$, length 1..1024), found {invalid:?}"
            ));
        }
    }
    if let Some(ou_scope) = scope.organizational_unit_scope.as_ref() {
        let listed = ou_scope.organizational_units.as_deref().unwrap_or_default();
        if ou_scope.all_organizational_units_enabled && !listed.is_empty() {
            return Some(
                "OrganizationalUnitScope$AllOrganizationalUnitsEnabled and \
                 OrganizationalUnitScope$OrganizationalUnits cannot both be specified: either \
                 enable management of all organizational units or list the ones to manage"
                    .to_string(),
            );
        }
        if ou_scope.all_organizational_units_enabled
            && ou_scope.exclude_specified_organizational_units
        {
            return Some(
                "OrganizationalUnitScope$AllOrganizationalUnitsEnabled and \
                 OrganizationalUnitScope$ExcludeSpecifiedOrganizationalUnits cannot both be \
                 specified: either enable management of all organizational units or exclude the \
                 ones in OrganizationalUnitScope$OrganizationalUnits"
                    .to_string(),
            );
        }
    }
    if let Some(policy_type_scope) = scope.policy_type_scope.as_ref() {
        let names_third_party = policy_type_scope
            .policy_types
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|policy_type| policy_type == SECURITY_SERVICE_THIRD_PARTY_FIREWALL);
        if names_third_party {
            return Some(format!(
                "PolicyTypeScope$PolicyTypes may not name \
                 {SECURITY_SERVICE_THIRD_PARTY_FIREWALL}: third-party policy types can only be \
                 managed by the Firewall Manager default administrator"
            ));
        }
    }
    None
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsFmsAwsfms20180101State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsFmsAwsfms20180101State>)
}

/// The longest Region name [`is_region_name`] accepts.
///
/// An EMULATOR-CHOSEN bound, generous against the Region names AWS publishes:
/// the longest of them (`ap-southeast-7`, `us-gov-east-1`) is half this. It is
/// what keeps the Region a request addresses — a value read out of the request's
/// own credential scope, and so a value its sender chooses — from sizing
/// anything the service derives from it: the Region segment of a minted ARN, a
/// refusal's message, and the key a per-Region record is held under
/// ([`ComAmazonawsFmsAwsfms20180101State::notification_channels`]).
const MAX_REGION_NAME_LEN: usize = 32;

/// Whether `value` has the form of an AWS Region name: lowercase letters, digits
/// and hyphens, at most [`MAX_REGION_NAME_LEN`] of them.
///
/// Every Region this emulator can address has that form, and so does the Region
/// segment of every ARN it mints, so a credential scope carrying anything else
/// names no Region this service could answer for.
fn is_region_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REGION_NAME_LEN
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The Region this request was signed for, which is the Region whose resources
/// the request addresses — so it is the Region segment of any ARN the response
/// carries. Falls back to [`DEFAULT_REGION`] for an unsigned request, and for a
/// credential scope whose Region is not a Region name ([`is_region_name`]):
/// such a scope addresses no Region this service serves, exactly as an unsigned
/// request names none.
///
/// Reading a scope that names no Region as "no Region named" is deliberately the
/// SAME reading as an unsigned request's, so two such scopes address the Region
/// a request naming none addresses rather than each keying state of its own.
/// Whether a presented scope may sign a request at all is the emulator's own
/// check, ahead of every handler — a service never inspects a signature — so
/// there is nothing here to refuse it with.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_name(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a Firewall Manager policy:
/// `arn:aws:fms:<region>:<account>:policy/<PolicyId>`.
pub fn policy_arn(region: &str, policy_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("policy/{policy_id}"))
}

/// The ARN of a Firewall Manager resource set:
/// `arn:aws:fms:<region>:<account>:resource-set/<Id>`.
pub fn resource_set_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("resource-set/{id}"))
}

/// The number of items ONE call may carry when it changes a resource set's
/// membership.
///
/// An EMULATOR-CHOSEN bound, not an AWS-documented one: the model's
/// `IdentifierList` declares no length, so the request boundary admits any
/// number of items and the service is what bounds them. It bounds the work ONE
/// call does — the classification loop runs under this service's state lock — so
/// a caller cannot make one request hold the store for an unbounded time. A
/// caller with more resources to associate or disassociate sends more calls.
///
/// Shared by both batch operations, so neither of them can admit a batch the
/// other refuses.
pub const MAX_RESOURCE_SET_ITEMS_PER_CALL: usize = 100;

/// Resolve a `ResourceSetIdentifier` to the key a resource set is stored under
/// in [`ComAmazonawsFmsAwsfms20180101State::resource_sets`].
///
/// The modelled `Identifier` is up to 2048 characters where the resource set's
/// own `Id` is 22, so a caller may name the set by either its `Id` or its ARN;
/// both are identifiers it holds. `None` when it names neither.
///
/// Lives here rather than in one operation so every operation that takes a
/// `ResourceSetIdentifier` resolves it identically.
pub fn resolve_resource_set_id(
    store: &ComAmazonawsFmsAwsfms20180101State,
    identifier: &str,
) -> Option<String> {
    if store.resource_sets.contains_key(identifier) {
        return Some(identifier.to_string());
    }
    store
        .resource_sets
        .values()
        .find(|record| record.arn == identifier)
        .map(|record| record.id.clone())
}

/// What a batch membership call classifies its items against: the resource
/// set's own declared `ResourceTypeList` and the membership it currently holds,
/// both read ONCE from the stored record.
#[derive(Debug, Clone)]
pub struct ResourceSetBatchBasis {
    /// The set's declared `ResourceTypeList`, the catalog
    /// [`resource_set_item_failure_reason`] classifies an item against.
    pub resource_types: Vec<String>,
    /// The resource URIs the set holds right now.
    pub held: BTreeSet<String>,
}

/// Re-read the resource set `id` names under the lock the caller already holds
/// and hand back the basis a batch membership call classifies against.
///
/// The step both batch membership operations have to take between resolving the
/// `ResourceSetIdentifier` ([`resolve_resource_set_id`]) and classifying their
/// items, single-homed here like every other rule those two share
/// ([`MAX_RESOURCE_SET_ITEMS_PER_CALL`], [`resource_set_item_failure_reason`],
/// [`ResourceSetRecord::set_resources`]), so neither of them can classify a
/// batch differently from the other.
///
/// A record the identifier resolved to is found under the same held lock, so
/// the miss is the service's own not-found answer rather than a default: no
/// future change can classify a batch against an empty `ResourceTypeList`, in
/// which [`resource_set_item_failure_reason`] fails every item. The caller
/// lifts the returned exception into its own modelled `ResourceNotFoundException`
/// variant.
pub fn resource_set_batch_basis(
    store: &ComAmazonawsFmsAwsfms20180101State,
    id: &str,
) -> Result<ResourceSetBatchBasis, ResourceNotFoundException> {
    let Some(record) = store.resource_sets.get(id) else {
        return Err(resource_not_found());
    };
    Ok(ResourceSetBatchBasis {
        resource_types: record.resource_set.resource_type_list.clone(),
        held: record.resources.clone(),
    })
}

/// Make `resources` the membership of the resource set `id` names, through the
/// record's own single writer ([`ResourceSetRecord::set_resources`]), so the
/// set's `LastUpdateTime` moves exactly when its membership changed.
///
/// The write half of [`resource_set_batch_basis`], single-homed for the same
/// reason: a record the identifier resolved to is still there under the same
/// held lock, so the miss is the service's own not-found answer rather than a
/// write that is silently dropped. Whether the membership CHANGED is the
/// record's own writer's answer, for a caller that has something to do with it;
/// the batch operations do not, so this reports only whether the write landed.
pub fn write_resource_set_resources(
    store: &mut ComAmazonawsFmsAwsfms20180101State,
    id: &str,
    resources: BTreeSet<String>,
    now: SystemTime,
) -> Result<(), ResourceNotFoundException> {
    let Some(record) = store.resource_sets.get_mut(id) else {
        return Err(resource_not_found());
    };
    record.set_resources(resources, now);
    Ok(())
}

/// Compare two names that spell the same thing differently on the wire: the
/// `AWS::NetworkFirewall::Firewall` resource type's `Firewall` and the
/// `firewall/<name>` of a Network Firewall ARN, or `ResolverQueryLogConfig` and
/// `resolver-query-log-config`. Keeps only the alphanumerics, lowercased.
fn normalized_type_name(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Every segment of an ARN's resource field that can NAME its type: each `/`- or
/// `:`-separated non-empty segment except the LAST, which is the resource's own
/// id — `firewall` of `firewall/<name>`, `instance` of `instance/i-0123`,
/// `restapis` and `stages` of `/restapis/<id>/stages/<name>`. The whole field
/// when it carries no separator, which is a resource whose ARN names only an id.
///
/// A service writes its type keyword wherever its own ARN form puts it, which is
/// not always the first segment (a path-shaped resource field puts it after the
/// parent resource it hangs off, and may begin with the separator itself), so the
/// type is looked for in all of them rather than only in the leading one.
///
/// What this walks is BOUNDED by the request boundary: an item is one
/// `IdentifierList` member, whose modelled length is 1..2048
/// (`smithy_query(query="shape",shape="IdentifierList")`), and a call carries at
/// most [`MAX_RESOURCE_SET_ITEMS_PER_CALL`] of them.
fn arn_resource_type_segments(resource: &str) -> impl Iterator<Item = &str> {
    let mut segments: Vec<&str> = resource
        .split(['/', ':'])
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() > 1 {
        segments.pop();
    }
    segments.into_iter()
}

/// The comparable form of a resource-type keyword: [`normalized_type_name`]
/// without a trailing plural `s`, so a resource field that writes its type in the
/// plural (`.../stages/<name>`) names the same type as the singular last segment
/// of an `AWS::<Service>::<Type>` name (`Stage`).
///
/// The `s` is dropped from BOTH sides of the comparison, so a type name that ends
/// in `s` for its own reasons loses it too: that keeps such a name matching
/// itself, and the ARN spelling of a type the rule cannot reach this way is what
/// [`ARN_RESOURCE_TYPE_KEYWORDS`] carries.
fn type_keyword(value: &str) -> String {
    let normalized = normalized_type_name(value);
    normalized.strip_suffix('s').unwrap_or(&normalized).to_string()
}

/// The keyword an ARN gives a resource type where the owning service spells it
/// differently from the last segment of the type's `AWS::<Service>::<Type>`
/// name, so the two cannot be matched by spelling alone.
///
/// SOURCE: the ARN form the service that owns the resource writes — Amazon EC2
/// addresses an Elastic IP by its allocation, `elastic-ip/<AllocationId>`, while
/// the resource type is named `AWS::EC2::EIP`. One entry per type whose two
/// spellings differ; a type absent from this table is spelled the same way in
/// both, so nothing needs to be said about it here.
const ARN_RESOURCE_TYPE_KEYWORDS: &[(&str, &str)] = &[(RESOURCE_TYPE_EC2_EIP, "elastic-ip")];

/// The type keywords an ARN's resource field names, as
/// [`arn_resource_type_segments`] reads them and [`type_keyword`] compares them.
///
/// Read ONCE per item, so an item is split and normalised once however many
/// resource types it is weighed against: the per-item work is the length of its
/// own resource field PLUS the number of declared types, never their product.
fn arn_type_keywords(resource: &str) -> Vec<String> {
    arn_resource_type_segments(resource)
        .map(type_keyword)
        .collect()
}

/// Whether `keywords` — the type keywords of an ARN's resource field, from
/// [`arn_type_keywords`] — name a resource of the type `declared` names, which is
/// a `Policy$ResourceType` value in the format of the Amazon Web Services
/// Resource Types Reference (`AWS::<Service>::<Type>`).
///
/// The type is read off the ARN the way the service that owns the resource writes
/// it: the keyword may sit at any segment of the resource field, may be written
/// in the plural, and may be spelled differently from the resource-type name
/// ([`ARN_RESOURCE_TYPE_KEYWORDS`]) — so an ARN the discovery inventory reports
/// for a type matches that type.
fn keywords_name_resource_type(keywords: &[String], declared: &str) -> bool {
    let declared_last = declared.rsplit("::").next().unwrap_or(declared);
    let names = |wanted: &str| keywords.iter().any(|keyword| keyword == wanted);
    names(&type_keyword(declared_last))
        || ARN_RESOURCE_TYPE_KEYWORDS
            .iter()
            .filter(|(resource_type, _)| {
                normalized_type_name(resource_type) == normalized_type_name(declared)
            })
            .any(|(_, keyword)| names(&type_keyword(keyword)))
}

/// The ARN service namespace of a resource type whose owning service publishes a
/// namespace spelled differently from the SERVICE segment of the type's own
/// `AWS::<Service>::<Type>` name, so the two cannot be matched by spelling
/// alone — the service-field twin of [`ARN_RESOURCE_TYPE_KEYWORDS`].
///
/// SOURCE: the namespace the owning service publishes as its `arnNamespace`.
/// Elastic Load Balancing serves BOTH load balancer generations under the one
/// `elasticloadbalancing` namespace
/// (`smithy_query(query="service_info")` on either Elastic Load Balancing
/// model, and the namespace this emulator's own Elastic Load Balancing v2 crate
/// mints its ARNs under), while the v2 resource type is named
/// `AWS::ElasticLoadBalancingV2::LoadBalancer`. One entry per type whose two
/// spellings differ; a type absent from this table is spelled the same way in
/// both, so nothing needs to be said about it here.
const ARN_RESOURCE_TYPE_NAMESPACES: &[(&str, &str)] =
    &[(RESOURCE_TYPE_ELBV2_LOAD_BALANCER, "elasticloadbalancing")];

/// The service a resource type belongs to, as the type's own name states it: the
/// MIDDLE segment of an `AWS::<Service>::<Type>` name (`EC2` of
/// `AWS::EC2::Instance`). `None` for a value that is not in that form, which
/// states no owning service at all.
fn resource_type_service(declared: &str) -> Option<&str> {
    let mut segments = declared.split("::");
    let (_vendor, service, _type) = (segments.next()?, segments.next()?, segments.next()?);
    (!service.is_empty()).then_some(service)
}

/// Whether `service` — the SERVICE field of an item's ARN — is the namespace of
/// the service that owns resources of the type `declared` names, which is a
/// `Policy$ResourceType` value in the format of the Amazon Web Services Resource
/// Types Reference (`AWS::<Service>::<Type>`).
///
/// A resource type names exactly one owning service, and every ARN of a resource
/// of that type carries that service's namespace in its third field, so the type
/// and the service field of an item's ARN are two readings of the SAME fact: the
/// type's service segment compared the way the two spellings differ
/// ([`normalized_type_name`] drops the punctuation an ARN namespace writes, so
/// `network-firewall` names `AWS::NetworkFirewall::Firewall`), plus the table of
/// namespaces a service publishes differently from its type names
/// ([`ARN_RESOURCE_TYPE_NAMESPACES`]).
///
/// A declared value that states no owning service ([`resource_type_service`])
/// names no namespace to disagree with, exactly as an ARN field the owning
/// service leaves empty names no value to disagree with, so it refuses nothing
/// on the service field.
fn arn_service_names_resource_type(service: &str, declared: &str) -> bool {
    let Some(declared_service) = resource_type_service(declared) else {
        return true;
    };
    let names = |candidate: &str| normalized_type_name(candidate) == normalized_type_name(service);
    names(declared_service)
        || ARN_RESOURCE_TYPE_NAMESPACES
            .iter()
            .filter(|(resource_type, _)| {
                normalized_type_name(resource_type) == normalized_type_name(declared)
            })
            .any(|(_, namespace)| names(namespace))
}

/// Why `uri` cannot be a member of a resource set that holds `resource_types`
/// in `region`, or `None` when it can.
///
/// Every reason is a value of the model's `FailedItemReason`
/// (`crate::ops::types::FAILED_ITEM_REASON_VALUES`), and the checks run from the
/// field that places the resource widest to the field that places it narrowest,
/// so an item is reported against the FIRST field that disagrees:
///
/// * not an ARN at all -> `NOT_VALID_ARN`;
/// * another partition than the one this emulator's resources live in
///   (`partition`) -> `NOT_VALID_PARTITION`;
/// * another Region than the one the request addresses -> `NOT_VALID_REGION`,
///   because a resource set holds the resources of the administrator's Region;
/// * an account that is not in this organization (`accounts`) ->
///   `NOT_VALID_ACCOUNT_ID`;
/// * a resource whose type no entry of the set's `ResourceTypeList` names ->
///   `NOT_VALID_RESOURCE_TYPE`, because the set's `ResourceTypeList`
///   "determines the resources that can be associated to the resource set"
///   (`ResourceSet$ResourceTypeList`);
/// * a resource whose type the set DOES name, under the namespace of a service
///   that does not own resources of that type -> `NOT_VALID_SERVICE`.
///
/// The last two are ONE reading of the declared types rather than two: a
/// `ResourceTypeList` entry names both the type and the service that owns
/// resources of it, and every ARN of such a resource carries that service's
/// namespace in its service field, so an item is in scope only where ONE
/// declared type answers for both of its fields
/// ([`keywords_name_resource_type`], [`arn_service_names_resource_type`]) — an
/// ARN naming a declared type under another service's namespace names a resource
/// that service does not hold. The item is reported against the broader
/// disagreement first: the type, which no declared entry names at all, before
/// the service field of an item whose type one of them does name.
///
/// An ARN field a service LEAVES EMPTY names no value to disagree with, so it is
/// no reason to refuse the item: a global service's ARN carries no Region and its
/// resource belongs to every Region, and an account-less service's ARN carries no
/// account segment to weigh against the organization's accounts. The type is read
/// off the ARN as the owning service writes it
/// ([`keywords_name_resource_type`]).
/// Those are the same readings the discoverable inventory makes
/// ([`in_region`], [`discovered_resources`]), which is what keeps a resource
/// reported as "available to be associated with a resource set" one these
/// operations actually take: that inventory reads each type from the store of the
/// service that OWNS it, so every URI it yields carries that service's namespace
/// and its own declared type.
///
/// `partition` and `accounts` are derived ONCE by the caller, so classifying a
/// batch does not rebuild them per item. It is one shared classifier so the
/// operations that associate and disassociate a resource's membership cannot
/// disagree about which items a set can hold.
pub fn resource_set_item_failure_reason(
    uri: &str,
    region: &str,
    partition: &str,
    accounts: &BTreeSet<String>,
    resource_types: &[String],
) -> Option<&'static str> {
    let Some(arn) = parse_arn(uri) else {
        return Some("NOT_VALID_ARN");
    };
    if arn.partition != partition {
        return Some("NOT_VALID_PARTITION");
    }
    if !arn_in_region(&arn, region) {
        return Some("NOT_VALID_REGION");
    }
    if !arn_in_accounts(&arn, accounts) {
        return Some("NOT_VALID_ACCOUNT_ID");
    }
    // The item's own keywords are read once, before the declared types are walked,
    // so classifying one item against a set's `ResourceTypeList` costs its ARN
    // plus that list rather than one times the other.
    let keywords = arn_type_keywords(arn.resource);
    // One declared type has to answer for BOTH fields the type decides: the
    // resource field's keyword and the service field's namespace.
    let in_scope = resource_types.iter().any(|declared| {
        keywords_name_resource_type(&keywords, declared)
            && arn_service_names_resource_type(arn.service, declared)
    });
    if in_scope {
        return None;
    }
    // Only a refused item is weighed a second time, to say WHICH of the two the
    // declared types disagree with it on.
    let names_a_declared_type = resource_types
        .iter()
        .any(|declared| keywords_name_resource_type(&keywords, declared));
    Some(if names_a_declared_type {
        "NOT_VALID_SERVICE"
    } else {
        "NOT_VALID_RESOURCE_TYPE"
    })
}

/// Whether `arn` names a resource of `region`: its Region field is `region`, or
/// it carries no Region at all — a global service's ARN (CloudFront's, for one)
/// leaves that field empty and its resource belongs to every Region.
///
/// The one Region reading this service makes of an ARN, shared by the
/// discoverable inventory ([`in_region`]) and by the item classifier
/// ([`resource_set_item_failure_reason`]) so the two cannot disagree.
fn arn_in_region(arn: &ParsedArn<'_>, region: &str) -> bool {
    arn.region.is_empty() || arn.region == region
}

/// Whether `arn` names a resource of one of `accounts`: its account field names
/// one of them, or it carries no account at all — an account-less service's ARN
/// (API Gateway's, for one) leaves that field empty, so it carries no account to
/// weigh and the resource belongs to whichever account of the organization holds
/// it.
fn arn_in_accounts(arn: &ParsedArn<'_>, accounts: &BTreeSet<String>) -> bool {
    arn.account.is_empty() || accounts.contains(arn.account)
}

/// The account a resource described by nothing but its ARN BELONGS TO: the
/// account field of that ARN where the owning service writes one, and the
/// account this emulator holds the resource in ([`emulator_owner`]) where that
/// service leaves the field empty — a global service's ARN (CloudFront's, S3's)
/// and an account-less service's (API Gateway's) carry no account, and a
/// resource whose URI names none still belongs to the account of the
/// organisation that holds it, which is the one this emulator presents.
///
/// The ONE owner reading this service makes of an ARN, shared by every surface
/// that reports an owner for a resource it knows only by URI — the membership
/// reader's `Resource$AccountId` — so it cannot disagree with the owner the
/// discoverable inventory reports for the same resource as
/// `DiscoveredResource$AccountId` ([`discovered_resources`], which reports this
/// emulator's account for exactly those account-less and global URIs). It is
/// the owner half of the same empty-field reading the item classifier makes
/// ([`arn_in_accounts`]): an empty account field weighs against nothing, which
/// is what makes such a resource associable in the first place.
///
/// `None` only where no `AWSAccountId` can be reported at all: a value that is
/// no ARN, and an ARN whose account field carries something that is not an
/// account id ([`is_account_id`] — `arn:aws:iam::aws:policy/...` names the
/// service itself there), because the modelled member's pattern (`^[0-9]+$`)
/// admits nothing else and the member is optional.
pub fn arn_owner_account(uri: &str) -> Option<String> {
    let arn = parse_arn(uri)?;
    if arn.account.is_empty() {
        return Some(emulator_owner());
    }
    is_account_id(arn.account).then(|| arn.account.to_string())
}

/// The accounts of this organization a resource may belong to: the ones
/// [`ComAmazonawsFmsAwsfms20180101State::member_accounts`] reports — the SAME
/// definition `ListMemberAccounts` answers with, so two operations cannot
/// disagree about who is in the organization — together with `discoverable`, the
/// accounts the discovered-resource inventory reports resources in, which are by
/// construction accounts Firewall Manager discovered resources in.
///
/// `discoverable` is [`discoverable_accounts`], which its caller reads BEFORE
/// taking this store's guard (that read takes the owning services' locks), so a
/// resource `ListDiscoveredResources` reports as discoverable is one the batch
/// operations accept from its account.
///
/// It takes `now` for the reason
/// [`ComAmazonawsFmsAwsfms20180101State::member_accounts`] does: one request
/// decides from ONE reading of a clock that is not monotonic, so the roster a
/// batch classifies its items against is the one its own gate was decided from.
pub fn organization_accounts(
    store: &ComAmazonawsFmsAwsfms20180101State,
    discoverable: BTreeSet<String>,
    now: SystemTime,
) -> BTreeSet<String> {
    let mut accounts = store.member_accounts(now);
    accounts.extend(discoverable);
    accounts
}

/// The ARN of a Firewall Manager applications list:
/// `arn:aws:fms:<region>:<account>:applications-list/<ListId>`.
pub fn apps_list_arn(region: &str, list_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("applications-list/{list_id}"),
    )
}

/// The ARN of a Firewall Manager protocols list:
/// `arn:aws:fms:<region>:<account>:protocols-list/<ListId>`.
pub fn protocols_list_arn(region: &str, list_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("protocols-list/{list_id}"))
}

/// The fields of an AWS ARN, `arn:<partition>:<service>:<region>:<account>:<resource>`,
/// as borrowed slices of the value that was parsed.
///
/// The resource field is everything after the fifth colon, so a resource id that
/// contains colons (`loadbalancer/app/x:1`) stays whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedArn<'a> {
    pub partition: &'a str,
    pub service: &'a str,
    pub region: &'a str,
    pub account: &'a str,
    pub resource: &'a str,
}

/// Which ONE condition of the ARN form ([`parse_arn_form`]) a value that is not
/// an ARN is the first to break.
///
/// Reported as a value rather than as a message so the single parse serves both
/// the callers that only ask whether a string is an ARN ([`parse_arn`]) and the
/// one that must word why it is not ([`arn_format_error`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArnFormDefect {
    /// The value does not begin with the literal `arn:` prefix.
    MissingPrefix,
    /// The value has fewer than six colon-separated fields (how many it has).
    TooFewFields(usize),
    /// A field the form requires to carry a value — `"partition"`, `"service"`
    /// or `"resource"` — is empty.
    EmptyField(&'static str),
}

impl ArnFormDefect {
    /// How this defect is worded for the caller that sent `arn`: one message per
    /// condition, naming the field or the count that disagrees and echoing the
    /// value, which is what the caller needs to correct the request.
    fn message(self, arn: &str) -> String {
        match self {
            Self::MissingPrefix => {
                format!("Incorrect arn format. error:ARNs must start with 'arn:': {arn}")
            }
            Self::TooFewFields(found) => format!(
                "Incorrect arn format. error:ARNs must have six colon-separated fields, \
                 found {found}: {arn}"
            ),
            Self::EmptyField(field) => {
                format!("Incorrect arn format. error:ARNs must name a {field}: {arn}")
            }
        }
    }
}

/// The ONE place this service states what makes a string an ARN: the parse of
/// `value` as the documented ARN layout
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>` — the literal
/// `arn` prefix, six colon-separated fields, and a non-empty partition, service
/// and resource. Region and account id are deliberately allowed to be empty —
/// ARNs of global and account-less resources leave them so.
///
/// It answers the parsed fields, or the first condition the value breaks, so the
/// operations that only need to know whether a value is an ARN ([`parse_arn`])
/// and the one that must answer WHY it is not ([`arn_format_error`]) read the
/// same rule rather than each re-deriving it: nothing can loosen the form for
/// the resource-set membership classifier while the tagging operations still
/// refuse the same string, or the other way round.
fn parse_arn_form(value: &str) -> Result<ParsedArn<'_>, ArnFormDefect> {
    if !value.starts_with("arn:") {
        return Err(ArnFormDefect::MissingPrefix);
    }
    // Six fields at most, so a resource id that itself contains colons stays
    // whole in the last one.
    let fields: Vec<&str> = value.splitn(6, ':').collect();
    let [_, partition, service, region, account, resource] = fields[..] else {
        return Err(ArnFormDefect::TooFewFields(fields.len()));
    };
    let parsed = ParsedArn {
        partition,
        service,
        region,
        account,
        resource,
    };
    match (parsed.partition, parsed.service, parsed.resource) {
        ("", _, _) => Err(ArnFormDefect::EmptyField("partition")),
        (_, "", _) => Err(ArnFormDefect::EmptyField("service")),
        (_, _, "") => Err(ArnFormDefect::EmptyField("resource")),
        _ => Ok(parsed),
    }
}

/// Parse an ARN, or `None` when `value` is not one at all — the form is the one
/// [`parse_arn_form`] states, for the callers that only need the fields and not
/// the reason a value has none.
pub fn parse_arn(value: &str) -> Option<ParsedArn<'_>> {
    parse_arn_form(value).ok()
}

/// The ARN partition this emulator's own resources live in, read off an ARN
/// built by [`engine::emulator_arn`] rather than written here as a literal, so
/// the partition has one source of truth.
pub fn emulator_partition() -> String {
    let minted = ::engine::emulator_arn(ARN_NAMESPACE, DEFAULT_REGION, "probe");
    parse_arn(&minted)
        .map(|arn| arn.partition.to_string())
        .unwrap_or_default()
}

/// The largest page any paginated operation of this service serves, which is
/// the maximum the model's `com.amazonaws.fms#PaginationMaxResults` declares for
/// `MaxResults` (range 1..100). It bounds the response for EVERY caller,
/// including one whose operation leaves `MaxResults` optional and sends none.
pub const MAX_PAGE_SIZE: usize = 100;

/// How many items one page carries for a request asking for `max_results`: the
/// ONE page-size rule every paginated operation of this service follows. A
/// request that names no bound gets [`MAX_PAGE_SIZE`]; one that names a bound
/// gets it clamped into `1..=MAX_PAGE_SIZE`.
///
/// Clamped rather than only capped: the modelled range already refuses a page
/// size below 1, and this keeps a page from ever being empty-with-a-token (which
/// a client would follow forever) if that validation ever changes.
///
/// An operation whose `MaxResults` member is REQUIRED passes
/// `Some(input.max_results)`, so the required and the optional member resolve
/// through the same rule and no two handlers can disagree about the bound the
/// model declares once.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results
        .map_or(MAX_PAGE_SIZE, |max| usize::try_from(max).unwrap_or(0))
        .clamp(1, MAX_PAGE_SIZE)
}

/// The collection `ListPolicies` pages, which
/// [`encode_policy_page_token`] stamps its boundaries with: the policies this
/// service stores ([`ComAmazonawsFmsAwsfms20180101State::policies`]).
pub const POLICIES_PAGE: &str = "ListPolicies";

/// Encode a page boundary of `ListPolicies` (the `PolicyId` the next page starts
/// at) as an opaque `PaginationToken`.
///
/// The stamp is what lets [`decode_policy_page_token`] tell a boundary of the
/// policies from one minted for ANOTHER collection this service keys by the same
/// minted-identifier class — the lists both `DefaultLists`-selected listings
/// enumerate, whose `ListId`s come from the same mint ([`mint_identifier`]) and
/// are therefore indistinguishable from a `PolicyId` by shape alone; the
/// encoding is [`encode_scoped_page_token`]'s, so these tokens stay inside the
/// modelled `PaginationToken` pattern and length and stay opaque to a caller.
pub fn encode_policy_page_token(policy_id: &str) -> String {
    encode_scoped_page_token(POLICIES_PAGE, policy_id)
}

/// Encode an arbitrary page boundary (the store key the next page starts at) as
/// an opaque `PaginationToken`, for the paginated operations whose key already
/// carries the collection it belongs to.
///
/// Hex keeps the token inside the modelled `PaginationToken` pattern
/// (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`) and its 1..4096 length whatever the key
/// contains, and nothing in it is meaningful to a caller without decoding.
pub fn encode_key_token(key: &str) -> String {
    ::hex::encode(key)
}

/// Decode a `PaginationToken` produced by [`encode_key_token`]. `None` when the
/// token was not produced by this service: a value that is not hex, not UTF-8,
/// empty, or carries a control character is no store key of ours.
///
/// Nor is a value whose SPELLING is not the one [`encode_key_token`] mints: hex
/// decoding admits both cases of the digits `a`-`f`, while the minter emits one
/// of them, so a case-variant of a token this service issued is a token it never
/// issued. The check is that re-encoding the decoded key reproduces the token
/// exactly — the only spelling a page boundary of this service has — which keeps
/// the accepted set equal to the minted set whatever the encoding is, rather
/// than restating the minter's alphabet here.
pub fn decode_key_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let is_key = !decoded.is_empty()
        && !decoded.chars().any(char::is_control)
        && encode_key_token(&decoded) == token;
    is_key.then_some(decoded)
}

/// Decode a `PaginationToken` produced by [`encode_policy_page_token`], yielding
/// the `PolicyId` the next page of the policies starts at.
///
/// `None` when the token was not produced by this service, or was produced for
/// ANOTHER collection: the token-to-key step is [`decode_scoped_page_token`]'s,
/// which answers only for a boundary stamped [`POLICIES_PAGE`], so a boundary of
/// a collection this service keys by the same identifier class names no page
/// here. A `ListId` of either list collection sorts arbitrarily among the stored
/// `PolicyId`s, so resuming from one would silently drop the policies below it
/// and report the remainder as a complete page.
///
/// The decoded boundary must still be a 36-character identifier this service
/// mints ([`is_minted_identifier`] — the form the model gives
/// `com.amazonaws.fms#PolicyId`), the shape check this page class adds on top of
/// the shared token-to-key step.
pub fn decode_policy_page_token(token: &str) -> Option<String> {
    let decoded = decode_scoped_page_token(POLICIES_PAGE, token)?;
    is_minted_identifier(&decoded).then_some(decoded)
}

/// Whether `value` has the form of an identifier this service mints: 36
/// characters matching `^[a-z0-9A-Z-]{36}$`, which the model gives both
/// `com.amazonaws.fms#PolicyId` and `com.amazonaws.fms#ListId` — so one check
/// serves the policy-keyed and the list-keyed pages.
pub fn is_minted_identifier(value: &str) -> bool {
    value.len() == 36
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Decode a `Base62Id`-keyed `PaginationToken` produced by
/// [`encode_key_token`], for the paginated operations whose store key is a
/// resource set's `Id`. `None` when the token was not produced by this service
/// for such a page.
///
/// The decoded value must still be a `Base62Id` (the model's
/// `com.amazonaws.fms#Base62Id`: exactly 22 characters matching
/// `^[a-z0-9A-Z]{22}$`), so a token minted for a policy or account page — or any
/// foreign token that happens to be valid hex — is refused rather than resumed
/// from as an arbitrary key.
pub fn decode_base62_page_token(token: &str) -> Option<String> {
    let decoded = decode_key_token(token)?;
    let is_base62_id = decoded.chars().count() == 22
        && decoded.chars().all(|c| c.is_ascii_alphanumeric());
    is_base62_id.then_some(decoded)
}

/// The page scope a boundary of the discoverable inventory is stamped with: the
/// THREE values a `ListDiscoveredResources` request selects its page by — the
/// Region it addresses, the member account it names and the resource type it
/// asks for, which is exactly what [`discovered_resources`] builds the page
/// from.
///
/// All three are needed, and no two of them identify the page: each resource
/// type is a different collection of URIs, each member account owns a different
/// one again, and the same account's resources in another Region are reported
/// under ARNs naming that Region. None of the three can contain the separator
/// [`scoped_page_key`] joins with — a Region and an `AWSAccountId` (`^[0-9]+$`)
/// carry no `#`, and neither does a `ResourceType`
/// (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`) — so the stamp is unambiguous.
fn discovered_resources_page_scope(region: &str, account: &str, resource_type: &str) -> String {
    scoped_page_key(region, &scoped_page_key(account, resource_type))
}

/// Encode a page boundary (the resource URI the next page starts at) of the
/// inventory one `ListDiscoveredResources` request selects, as an opaque
/// `PaginationToken`.
///
/// The stamp is what lets [`decode_discovered_resources_page_token`] tell a
/// boundary of THIS selection's page from one minted for another account,
/// another resource type or another Region; the encoding is
/// [`encode_scoped_page_token`]'s, so these tokens stay inside the modelled
/// `PaginationToken` pattern and length and stay opaque to a caller.
pub fn encode_discovered_resources_page_token(
    region: &str,
    account: &str,
    resource_type: &str,
    uri: &str,
) -> String {
    encode_scoped_page_token(
        &discovered_resources_page_scope(region, account, resource_type),
        uri,
    )
}

/// Decode a `PaginationToken` produced by
/// [`encode_discovered_resources_page_token`] for the inventory the Region,
/// member account and resource type select, yielding the URI the next page
/// resumes at.
///
/// `None` when the token was not produced by this service, or was produced for
/// ANOTHER selection of the inventory: a URI of another selection's page is
/// either absent from this one or sorts past resources this one does hold, so
/// resuming from it would silently drop them and report a short page — or an
/// empty one — as a complete page.
///
/// The decoded boundary must still be an ARN ([`parse_arn`]), the shape check
/// this page class adds on top of the shared token-to-key step: a discovered
/// resource is reported under the ARN it would be associated by, so nothing
/// else is a key of this page.
pub fn decode_discovered_resources_page_token(
    region: &str,
    account: &str,
    resource_type: &str,
    token: &str,
) -> Option<String> {
    let decoded = decode_scoped_page_token(
        &discovered_resources_page_scope(region, account, resource_type),
        token,
    )?;
    parse_arn(&decoded).is_some().then_some(decoded)
}

/// The page scope [`encode_account_page_token`] stamps a
/// `ListAdminAccountsForOrganization` boundary with: the collection that
/// listing enumerates is the organisation's administrators
/// ([`ComAmazonawsFmsAwsfms20180101State::admin_account_summaries`]).
pub const ADMIN_ACCOUNTS_FOR_ORGANIZATION_PAGE: &str = "ListAdminAccountsForOrganization";

/// The page scope [`encode_account_page_token`] stamps a `ListMemberAccounts`
/// boundary with: the collection that listing enumerates is every account the
/// organisation holds
/// ([`ComAmazonawsFmsAwsfms20180101State::member_accounts`]), a strict superset
/// of the administrators.
pub const MEMBER_ACCOUNTS_PAGE: &str = "ListMemberAccounts";

/// The page scope [`encode_account_page_token`] stamps a
/// `ListAdminsManagingAccount` boundary with: the collection that listing
/// enumerates is the administrators that manage the caller
/// ([`ComAmazonawsFmsAwsfms20180101State::managing_admin_accounts`]).
pub const ADMINS_MANAGING_ACCOUNT_PAGE: &str = "ListAdminsManagingAccount";

/// Encode an account-keyed page boundary of the collection `scope` names as an
/// opaque `PaginationToken`.
///
/// Three listings of this service page an account-keyed collection, and those
/// collections are different sets, so an account id alone names no page: the
/// same id may be absent from another collection, or sort after entries that
/// collection does hold, and resuming from it there would silently drop them.
/// Carrying the collection beside the key is what lets
/// [`decode_account_page_token`] tell a boundary of THIS collection from one of
/// another — the same discipline the listing whose store key already begins
/// with the collection it belongs to applies to that key's prefix
/// ([`third_party_firewall_policy_key_prefix`]).
pub fn encode_account_page_token(scope: &str, account_id: &str) -> String {
    encode_scoped_page_token(scope, account_id)
}

/// Decode an account-keyed `PaginationToken` produced by
/// [`encode_account_page_token`] for the collection `scope` names, yielding the
/// account id the next page starts at. `None` when the token was not produced
/// by this service, or was produced for ANOTHER collection: either way it names
/// no boundary of this page.
///
/// The token-to-key step is [`decode_scoped_page_token`]'s, like every other
/// specialised decoder here, so this module expresses that rule once; this one
/// adds only its own shape check.
///
/// The decoded boundary must still be an `AWSAccountId` ([`is_account_id`]), so
/// a token minted for a page of another key class — or any foreign token that
/// happens to be valid hex — is refused rather than resumed from as an
/// arbitrary key.
pub fn decode_account_page_token(scope: &str, token: &str) -> Option<String> {
    let decoded = decode_scoped_page_token(scope, token)?;
    is_account_id(&decoded).then_some(decoded)
}

/// Join the value a page was SELECTED by to a boundary inside it, the way this
/// service's composite store keys are built (see [`shield_attack_key`]): the
/// selecting value, the separator, then the key. Neither a `PolicyId` nor a
/// `Base62Id` may contain the separator, so the join is unambiguous.
fn scoped_page_key(scope: &str, key: &str) -> String {
    format!("{scope}#{key}")
}

/// Encode a page boundary of a listing whose page set is one of SEVERAL this
/// service pages by the same key class — the collection a request member
/// SELECTS (the accounts one policy applies to, the members of one resource set,
/// the lists of one namespace), or the collection ONE listing of a shared key
/// class enumerates ([`encode_account_page_token`]) — as an opaque
/// `PaginationToken` carrying the collection beside the boundary.
///
/// A boundary of such a page names nothing in another collection: the key may be
/// absent there, or sort after items that collection does hold, so resuming from
/// it would silently drop them. Carrying the collection is what lets
/// [`decode_scoped_page_token`] tell a token minted for THIS page from one
/// minted for another — the same check the listing whose store key already
/// begins with the selecting member makes against that key's prefix
/// ([`third_party_firewall_policy_key_prefix`]).
///
/// Hex for the same reasons as [`encode_key_token`], which does the encoding:
/// the token stays inside the modelled `PaginationToken` pattern and length, and
/// nothing in it is meaningful to a caller without decoding.
pub fn encode_scoped_page_token(scope: &str, key: &str) -> String {
    encode_key_token(&scoped_page_key(scope, key))
}

/// Decode a `PaginationToken` produced by [`encode_scoped_page_token`] for the
/// page `scope` names, yielding the boundary inside it. `None` when the token
/// was not produced by this service, or was produced for ANOTHER collection:
/// either way it names no boundary of this page, so the caller refuses it
/// instead of resuming from an arbitrary key.
///
/// The token-to-key step is [`decode_key_token`]'s, like every other specialised
/// decoder here, so this module expresses that rule once; this one adds only the
/// collection check and leaves the boundary's own shape to the page class
/// that asked ([`is_account_id`] for an account-keyed page, [`parse_arn`] for a
/// URI-keyed one, [`is_minted_identifier`] for one keyed by a minted id).
pub fn decode_scoped_page_token(scope: &str, token: &str) -> Option<String> {
    let decoded = decode_key_token(token)?;
    let key = decoded
        .strip_prefix(&scoped_page_key(scope, ""))?
        .to_string();
    (!key.is_empty()).then_some(key)
}

/// The collection `ListAppsLists` pages: the applications lists this service
/// stores ([`ComAmazonawsFmsAwsfms20180101State::apps_lists`]).
pub const APPS_LISTS_PAGE: &str = "ListAppsLists";

/// The collection `ListProtocolsLists` pages: the protocols lists this service
/// stores ([`ComAmazonawsFmsAwsfms20180101State::protocols_lists`]), a different
/// collection from the applications lists and keyed independently of them.
pub const PROTOCOLS_LISTS_PAGE: &str = "ListProtocolsLists";

/// The page one of the `DefaultLists`-selected listings serves: the namespace
/// that member selects INSIDE the collection `collection` names.
///
/// Both halves are needed, and neither alone identifies the page: the two
/// namespaces of one collection share its id space (so a namespace must be
/// named), and the two collections are enumerated by different listings whose
/// `ListId`s are minted independently of each other (so the collection must be
/// named too). Neither a collection name nor a boolean contains the separator
/// [`scoped_page_key`] joins with, so the stamp is unambiguous.
fn namespaced_list_page_scope(collection: &str, default_lists: bool) -> String {
    scoped_page_key(collection, &default_lists.to_string())
}

/// Encode a page boundary (the `ListId` the next page starts at) of the
/// namespace `default_lists` selects in the list collection `collection` names,
/// as an opaque `PaginationToken`.
///
/// The stamp is what lets [`decode_namespaced_list_page_token`] tell a boundary
/// of THIS page from one minted for the other namespace or for the other
/// collection; the encoding is [`encode_scoped_page_token`]'s, so these tokens
/// stay inside the modelled `PaginationToken` pattern and length and stay opaque
/// to a caller.
pub fn encode_namespaced_list_page_token(
    collection: &str,
    default_lists: bool,
    list_id: &str,
) -> String {
    encode_scoped_page_token(&namespaced_list_page_scope(collection, default_lists), list_id)
}

/// Decode a `PaginationToken` produced by [`encode_namespaced_list_page_token`]
/// for the namespace `default_lists` of the collection `collection` names,
/// yielding the `ListId` the next page starts at.
///
/// `None` when the token was not produced by this service, or was produced for
/// the other namespace or for the other list collection: a `ListId` of another
/// page is either absent from this one or sorts past lists this one does hold,
/// so resuming from it would silently drop them.
///
/// The decoded boundary must still be a `ListId` ([`is_minted_identifier`]), the
/// shape check this page class adds on top of the shared token-to-key step.
pub fn decode_namespaced_list_page_token(
    collection: &str,
    default_lists: bool,
    token: &str,
) -> Option<String> {
    let decoded =
        decode_scoped_page_token(&namespaced_list_page_scope(collection, default_lists), token)?;
    is_minted_identifier(&decoded).then_some(decoded)
}

/// A stored list of one of the collections a `DefaultLists`-selected listing
/// enumerates — an applications list or a protocols list.
///
/// It carries the ONE thing [`namespaced_list_page`] needs of such a record:
/// which namespace of its collection the record belongs to. The stored flag is
/// what tells a Firewall Manager MANAGED list from a customer one, the same
/// split [`ComAmazonawsFmsAwsfms20180101State::apps_list_in_namespace`] resolves
/// a single id in, so the page walk reads the namespace off the record exactly
/// as a reader of one list does.
pub trait NamespacedListRecord {
    /// Whether this list is a Firewall Manager MANAGED (default) list.
    fn default_list(&self) -> bool;
}

impl NamespacedListRecord for AppsListRecord {
    fn default_list(&self) -> bool {
        self.default_list
    }
}

impl NamespacedListRecord for ProtocolsListRecord {
    fn default_list(&self) -> bool {
        self.default_list
    }
}

/// One page of a `DefaultLists`-selected listing: the records it reports, in the
/// ascending `ListId` order the store keeps them in, and the `NextToken` the
/// following page resumes at — `None` on the last page.
#[derive(Debug)]
pub struct NamespacedListPage<'store, Record> {
    /// The records this page reports, at most [`page_size`] of them.
    pub records: Vec<&'store Record>,
    /// The boundary token of the following page, minted only when a list of
    /// this page is left after the records above.
    pub next_token: Option<String>,
}

/// Serve one page of the namespace `default_lists` selects inside the list
/// collection `collection` names, out of the `ListId`-keyed store `lists`.
///
/// The ONE copy of the page rule both `DefaultLists`-selected listings follow,
/// so neither can drift from the other: the page starts at the boundary
/// `next_token` names ([`decode_namespaced_list_page_token`]) or at the first
/// `ListId` when there is none, holds only the lists of the selected namespace,
/// is bounded by the shared [`page_size`] rule, and mints a boundary token of
/// its own ([`encode_namespaced_list_page_token`]) only when a list of this page
/// remains after it.
///
/// `None` when `next_token` names no boundary of THIS page — a token this
/// service did not mint, or one minted for the other namespace or the other
/// collection, whose `ListId` is either absent here or sorts past lists this
/// page does hold. The calling listing refuses such a request in its own words,
/// since each one names itself in its refusal.
pub fn namespaced_list_page<'store, Record: NamespacedListRecord>(
    lists: &'store BTreeMap<String, Record>,
    collection: &str,
    default_lists: bool,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Option<NamespacedListPage<'store, Record>> {
    // The boundary is a store key, so the page resumes at the first `ListId` at
    // or after it; with no token the lower bound is the empty id, which sorts
    // before every `ListId`.
    let lower = match next_token {
        Some(token) => decode_namespaced_list_page_token(collection, default_lists, token)?,
        None => String::new(),
    };
    let mut remaining = lists
        .range(lower..)
        .filter(|(_, record)| record.default_list() == default_lists);
    let records: Vec<&Record> = remaining
        .by_ref()
        .take(page_size(max_results))
        .map(|(_, record)| record)
        .collect();
    let next_token = remaining
        .next()
        .map(|(list_id, _)| encode_namespaced_list_page_token(collection, default_lists, list_id));
    Some(NamespacedListPage {
        records,
        next_token,
    })
}

/// Mint a fresh service-owned identifier: 36 lowercase hex characters, which
/// satisfies the model's `PolicyId` pattern (`^[a-z0-9A-Z-]{36}$`, length 36)
/// and also `PolicyUpdateToken`'s (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, length
/// 1..1024), so one mint serves both.
///
/// The value is the SHA-256 of a process-wide monotonic counter and the current
/// instant, hex-encoded and truncated to the modelled 36 characters: the counter
/// makes two mints in one process distinct whatever the clock does (so a rotated
/// `PolicyUpdateToken` always differs from the one it replaced, even within the
/// same nanosecond), and the instant separates two processes. Hashing and
/// hex-encoding are the `sha2` and `hex` crates' work, not hand-written here,
/// and nothing from the request goes into it.
pub fn mint_identifier() -> String {
    mint_hex(36)
}

/// Mint a fresh service-owned `Base62Id`: 22 characters matching the model's
/// `com.amazonaws.fms#Base62Id` (`^[a-z0-9A-Z]{22}$`, length 22), which is what
/// a resource set's `Id` is.
///
/// The same mint as [`mint_identifier`], cut to 22 characters: hex digits are a
/// subset of base62, so a hex string satisfies the base62 pattern.
pub fn mint_base62_id() -> String {
    mint_hex(22)
}

// ---------------------------------------------------------------------------
// The organisation's discoverable resource inventory
// ---------------------------------------------------------------------------
//
// Firewall Manager does not keep a registry of the organisation's resources: it
// DISCOVERS them from the accounts of the organisation, where the service that
// owns each kind of resource holds it. This emulator stands in for those
// services, so the inventory is read from THEIR stores through the one shared
// engine (the same cross-service read `ListS3Buckets` does for S3's buckets) —
// never from a second copy kept here, which nothing would write and which would
// disagree with the owning service the moment a resource was created or deleted.

/// A resource type Firewall Manager works with, "in the format shown in the
/// Amazon Web Services Resource Types Reference" (`Policy$ResourceType`).
/// [`DISCOVERABLE_RESOURCE_TYPES`] lists the ones this emulator can discover and
/// says where each catalogue entry comes from.
pub const RESOURCE_TYPE_APIGATEWAY_STAGE: &str = "AWS::ApiGateway::Stage";
/// See [`RESOURCE_TYPE_APIGATEWAY_STAGE`].
pub const RESOURCE_TYPE_CLOUDFRONT_DISTRIBUTION: &str = "AWS::CloudFront::Distribution";
/// See [`RESOURCE_TYPE_APIGATEWAY_STAGE`].
pub const RESOURCE_TYPE_EC2_EIP: &str = "AWS::EC2::EIP";
/// See [`RESOURCE_TYPE_APIGATEWAY_STAGE`].
pub const RESOURCE_TYPE_EC2_INSTANCE: &str = "AWS::EC2::Instance";
/// See [`RESOURCE_TYPE_APIGATEWAY_STAGE`].
pub const RESOURCE_TYPE_EC2_NETWORK_INTERFACE: &str = "AWS::EC2::NetworkInterface";
/// See [`RESOURCE_TYPE_APIGATEWAY_STAGE`].
pub const RESOURCE_TYPE_EC2_SECURITY_GROUP: &str = "AWS::EC2::SecurityGroup";
/// See [`RESOURCE_TYPE_APIGATEWAY_STAGE`].
pub const RESOURCE_TYPE_EC2_SUBNET: &str = "AWS::EC2::Subnet";
/// See [`RESOURCE_TYPE_APIGATEWAY_STAGE`].
pub const RESOURCE_TYPE_EC2_VPC: &str = "AWS::EC2::VPC";
/// See [`RESOURCE_TYPE_APIGATEWAY_STAGE`].
pub const RESOURCE_TYPE_ELB_LOAD_BALANCER: &str = "AWS::ElasticLoadBalancing::LoadBalancer";
/// See [`RESOURCE_TYPE_APIGATEWAY_STAGE`].
pub const RESOURCE_TYPE_ELBV2_LOAD_BALANCER: &str = "AWS::ElasticLoadBalancingV2::LoadBalancer";
/// See [`RESOURCE_TYPE_APIGATEWAY_STAGE`].
pub const RESOURCE_TYPE_NETWORK_FIREWALL_FIREWALL: &str = "AWS::NetworkFirewall::Firewall";

/// Every resource type this emulator can discover resources of, in ascending
/// order so the catalogue reads as a set.
///
/// SOURCE of the catalogue: the resource types `Policy$ResourceType` documents
/// as valid for a Firewall Manager policy type
/// (`smithy_query(query="doc",shape="Policy",member="ResourceType")`:
/// `AWS::ApiGateway::Stage`, `AWS::CloudFront::Distribution`,
/// `AWS::ElasticLoadBalancingV2::LoadBalancer`,
/// `AWS::ElasticLoadBalancing::LoadBalancer`, `AWS::EC2::EIP`,
/// `AWS::EC2::Subnet`, `AWS::EC2::SecurityGroup`, `AWS::EC2::NetworkInterface`,
/// `AWS::EC2::Instance` and `AWS::EC2::VPC`), together with
/// `AWS::NetworkFirewall::Firewall`, the Network Firewall resource a Firewall
/// Manager RESOURCE SET holds — the very thing `ListDiscoveredResources`
/// reports resources "available to be associated with" and the type
/// `BatchAssociateResource` already spells out.
///
/// A type NOT in this catalogue is one no service this emulator stands in for
/// holds resources of: it is reported as an EMPTY inventory, never refused and
/// never answered with an invented resource. Adding the service that owns such a
/// type is what adds its entry here.
pub const DISCOVERABLE_RESOURCE_TYPES: &[&str] = &[
    RESOURCE_TYPE_APIGATEWAY_STAGE,
    RESOURCE_TYPE_CLOUDFRONT_DISTRIBUTION,
    RESOURCE_TYPE_EC2_EIP,
    RESOURCE_TYPE_EC2_INSTANCE,
    RESOURCE_TYPE_EC2_NETWORK_INTERFACE,
    RESOURCE_TYPE_EC2_SECURITY_GROUP,
    RESOURCE_TYPE_EC2_SUBNET,
    RESOURCE_TYPE_EC2_VPC,
    RESOURCE_TYPE_ELB_LOAD_BALANCER,
    RESOURCE_TYPE_ELBV2_LOAD_BALANCER,
    RESOURCE_TYPE_NETWORK_FIREWALL_FIREWALL,
];

/// One resource an emulated service holds, as the inventory reads it out of that
/// service's own store: the URI it would be associated by (its ARN), the name it
/// carries, and the account that owns it.
#[derive(Debug, Clone)]
struct HeldResource {
    uri: String,
    name: String,
    account: String,
}

/// The account a resource belongs to when neither its own store nor its ARN
/// records an owner: the ONE account this emulator presents
/// ([`::engine::emulator_account_id`]), which is also the account segment of
/// every ARN [`::engine::emulator_arn`] builds.
///
/// Read both by the inventory, for a record whose owning service keeps no
/// `OwnerId`, and by [`arn_owner_account`], for an ARN whose owning service
/// leaves the account field empty — so the two report one owner for one
/// resource.
fn emulator_owner() -> String {
    ::engine::emulator_account_id().to_string()
}

/// The ARN of a resource of `service` in `region` that `account` OWNS, for a
/// record whose own store keeps no ARN.
///
/// [`::engine::emulator_arn`] stamps this emulator's own account into the ARN,
/// which is right for a resource this emulator owns and wrong for one whose
/// record reports another `OwnerId`: the reported `AccountId` and the account
/// segment of the reported `URI` must name the SAME account, or a resource set
/// would be offered a URI its account check refuses.
fn owned_arn(service: &str, region: &str, account: &str, resource: &str) -> String {
    format!(
        "arn:{}:{service}:{region}:{account}:{resource}",
        emulator_partition()
    )
}

/// Whether `uri` names a resource of `region`: an ARN whose Region field is
/// `region`, or one that carries no Region at all (a global service's ARN, such
/// as CloudFront's).
///
/// A value that is not an ARN names no resource that could be associated, so it
/// is not in any Region's inventory. The Region reading itself is
/// [`arn_in_region`], the one the item classifier makes too. The check is what
/// keeps the inventory and `BatchAssociateResource` in agreement: a resource set
/// holds the resources of the administrator's Region, so a resource reported as
/// discoverable in a Region is one that Region's resource sets can actually take.
fn in_region(uri: &str, region: &str) -> bool {
    parse_arn(uri).is_some_and(|arn| arn_in_region(&arn, region))
}

/// Whether `value` can be carried by the model's `ResourceName`
/// (`com.amazonaws.fms#ResourceName`: length 1..128, pattern
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`) — so a resource whose owning service lets
/// it carry a longer or otherwise unrepresentable name is reported WITHOUT a
/// `Name` rather than with one the model cannot hold.
///
/// The character classes are `char`'s own, which APPROXIMATE the pattern's
/// Unicode classes: `is_alphabetic` is a superset of `\p{L}` (it also admits
/// Other_Alphabetic marks), `is_numeric` of `\p{N}`, and
/// `is_whitespace() && !is_control()` of `\p{Z}`. The approximation is on the
/// permissive side and decides only whether `Name` is reported or omitted, never
/// whether a resource is discovered.
fn is_resource_name(value: &str) -> bool {
    (1..=128).contains(&value.chars().count())
        && value.chars().all(|c| {
            c.is_alphabetic()
                || c.is_numeric()
                // `\p{Z}` is the SEPARATOR category: a control character such as
                // a tab or a newline is whitespace but not a separator, and the
                // pattern does not admit it.
                || (c.is_whitespace() && !c.is_control())
                || matches!(c, '_' | '.' | ':' | '/' | '=' | '+' | '-' | '@')
        })
}

/// The resources Amazon EC2 holds of `resource_type`, read from the EC2
/// service's own store through the shared engine.
///
/// An EC2 resource's ARN is `arn:aws:ec2:<region>:<account>:<kind>/<id>`, which
/// is the URI a resource set associates it by; the stored ARN is preferred where
/// the record carries one (a subnet's `SubnetArn`, a security group's
/// `SecurityGroupArn`), so the Region a resource is discoverable in is the one
/// the service that created it recorded.
///
/// LOCK ORDER: this takes the EC2 store's lock, holds no other while it does,
/// and hands back owned values rather than a guard — so it is called with NO
/// Firewall Manager store guard held. Keeping the cross-service read
/// one-directional and guard-free is what makes a lock cycle unreachable.
fn ec2_resources<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    resource_type: &str,
) -> Vec<HeldResource> {
    let Some(state) = ::service_com_amazonaws_ec2_amazonec2::state::service_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    // The ARN of a resource whose own record keeps none. EC2's network interface
    // and address stores are NOT Region-keyed and those records carry no ARN, so
    // such a resource is reported in whichever Region asks — exactly as EC2's own
    // Region-agnostic describes report it, and so [`in_region`] does no filtering
    // for those two types. The Region filter bites for the types whose record DOES
    // keep an ARN (`SubnetArn`, `SecurityGroupArn`), and the VPC and instance arms
    // below narrow themselves against the Region EC2's own store records for each
    // (`vpc_is_in_region`, `instance_region`), which is what EC2's own describes of
    // those two read.
    //
    // The ARN's ACCOUNT field is the account the record itself reports as the
    // owner, not this emulator's: a resource whose `OwnerId` is another account
    // is reported with an `AccountId` and a `URI` that name the SAME account, so
    // the pair cannot disagree and the URI is one `BatchAssociateResource`
    // accepts from that account.
    let arn = |account: &str, resource: &str| owned_arn("ec2", region, account, resource);
    match resource_type {
        RESOURCE_TYPE_EC2_VPC => store
            .vpcs
            .values()
            .filter(|vpc| store.vpc_is_in_region(vpc.vpc_id.as_deref(), region))
            .filter_map(|vpc| {
                let id = vpc.vpc_id.clone()?;
                let account = vpc.owner_id.clone().unwrap_or_else(emulator_owner);
                Some(HeldResource {
                    uri: arn(&account, &format!("vpc/{id}")),
                    name: id,
                    account,
                })
            })
            .collect(),
        RESOURCE_TYPE_EC2_SUBNET => store
            .subnets
            .values()
            .filter_map(|subnet| {
                let id = subnet.subnet_id.clone()?;
                let account = subnet.owner_id.clone().unwrap_or_else(emulator_owner);
                Some(HeldResource {
                    uri: subnet
                        .subnet_arn
                        .clone()
                        .unwrap_or_else(|| arn(&account, &format!("subnet/{id}"))),
                    name: id,
                    account,
                })
            })
            .collect(),
        RESOURCE_TYPE_EC2_SECURITY_GROUP => store
            .security_groups
            .values()
            .filter_map(|group| {
                let id = group.group_id.clone()?;
                let account = group.owner_id.clone().unwrap_or_else(emulator_owner);
                Some(HeldResource {
                    uri: group
                        .security_group_arn
                        .clone()
                        .unwrap_or_else(|| arn(&account, &format!("security-group/{id}"))),
                    // A security group is addressed by its `GroupName` where it
                    // has one, which is the name AWS shows for it.
                    name: group.group_name.clone().unwrap_or(id),
                    account,
                })
            })
            .collect(),
        RESOURCE_TYPE_EC2_NETWORK_INTERFACE => store
            .network_interfaces
            .values()
            .filter_map(|interface| {
                let id = interface.network_interface_id.clone()?;
                let account = interface.owner_id.clone().unwrap_or_else(emulator_owner);
                Some(HeldResource {
                    uri: arn(&account, &format!("network-interface/{id}")),
                    name: id,
                    account,
                })
            })
            .collect(),
        RESOURCE_TYPE_EC2_INSTANCE => store
            .instances
            .values()
            .filter(|instance| {
                ::service_com_amazonaws_ec2_amazonec2::state::instance_region(instance) == region
            })
            .map(|instance| {
                let account = emulator_owner();
                HeldResource {
                    uri: arn(&account, &format!("instance/{}", instance.instance_id)),
                    name: instance.instance_id.clone(),
                    account,
                }
            })
            .collect(),
        // An Elastic IP address is associated by the ARN of its ALLOCATION; an
        // address held without an allocation id (an EC2-Classic address) names
        // no resource an ARN can address, so it is not discoverable.
        RESOURCE_TYPE_EC2_EIP => store
            .addresses
            .values()
            .filter_map(|address| {
                let id = address.allocation_id.clone()?;
                let account = emulator_owner();
                Some(HeldResource {
                    uri: arn(&account, &format!("elastic-ip/{id}")),
                    name: id,
                    account,
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The Application/Network/Gateway Load Balancers Elastic Load Balancing v2
/// holds, read from that service's own store. Each record keeps the ARN its
/// create minted, which carries the Region it was created in.
///
/// LOCK ORDER: as [`ec2_resources`].
fn load_balancers_v2<Eng: Send + Sync + 'static>(engine: &Eng) -> Vec<HeldResource> {
    let Some(state) =
        ::service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::service_state(
            engine,
        )
    else {
        return Vec::new();
    };
    let store = state.lock();
    store
        .load_balancers
        .values()
        .map(|record| HeldResource {
            uri: record.arn.clone(),
            name: record.name.clone(),
            account: emulator_owner(),
        })
        .collect()
}

/// The Classic Load Balancers Elastic Load Balancing holds IN `region` — that
/// store is keyed by Region first, a load balancer being a regional resource —
/// read from that service's own store.
///
/// LOCK ORDER: as [`ec2_resources`].
fn load_balancers_classic<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
) -> Vec<HeldResource> {
    let Some(state) =
        ::service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::state::service_state(
            engine,
        )
    else {
        return Vec::new();
    };
    let store = state.lock();
    store
        .load_balancers_in(region)
        .map(|held| {
            held.keys()
                .map(|name| HeldResource {
                    uri: ::engine::emulator_arn(
                        "elasticloadbalancing",
                        region,
                        &format!("loadbalancer/{name}"),
                    ),
                    name: name.clone(),
                    account: emulator_owner(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The CloudFront distributions that service holds, read from its own store.
/// CloudFront is global, so a distribution's stored ARN carries no Region and it
/// is discoverable from every Region.
///
/// LOCK ORDER: as [`ec2_resources`].
fn distributions<Eng: Send + Sync + 'static>(engine: &Eng) -> Vec<HeldResource> {
    let Some(state) =
        ::service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::service_state(engine)
    else {
        return Vec::new();
    };
    let store = state.lock();
    store
        .distributions
        .values()
        .map(|record| HeldResource {
            uri: record.arn.clone(),
            // A distribution has no name of its own; its `Id` is what names it.
            name: record.id.clone(),
            account: emulator_owner(),
        })
        .collect()
}

/// The API Gateway stages that service holds, read from its own store: one entry
/// per stage of every REST API, named by the stage.
///
/// An API Gateway ARN carries an EMPTY account field
/// (`arn:aws:apigateway:<region>::/restapis/<id>/stages/<name>`, built by that
/// service's own `state::service_arn`), so the owning account is this
/// emulator's rather than the URI's.
///
/// LOCK ORDER: as [`ec2_resources`].
fn api_gateway_stages<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
) -> Vec<HeldResource> {
    use ::service_com_amazonaws_apigateway_backplanecontrolservice::state as apigateway;

    let Some(state) = apigateway::service_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    store
        .rest_apis
        .iter()
        .flat_map(|(api_id, api)| {
            api.stages.keys().map(move |stage| HeldResource {
                uri: apigateway::service_arn(
                    region,
                    &format!("/restapis/{api_id}/stages/{stage}"),
                ),
                name: stage.clone(),
                account: emulator_owner(),
            })
        })
        .collect()
}

/// The Network Firewall firewalls that service holds, read from its own store.
/// Each record keeps the `FirewallArn` its create minted.
///
/// LOCK ORDER: as [`ec2_resources`].
fn firewalls<Eng: Send + Sync + 'static>(engine: &Eng) -> Vec<HeldResource> {
    let Some(state) =
        ::service_com_amazonaws_networkfirewall_networkfirewall_20201112::state::service_state(
            engine,
        )
    else {
        return Vec::new();
    };
    let store = state.lock();
    store
        .firewalls
        .values()
        .map(|record| HeldResource {
            uri: record.firewall_arn.clone(),
            name: record.firewall_name.clone(),
            account: emulator_owner(),
        })
        .collect()
}

/// Every resource this emulator holds of `resource_type`, from whichever service
/// owns that type — the catalogue [`DISCOVERABLE_RESOURCE_TYPES`] names. A type
/// no emulated service owns holds nothing.
fn held_resources<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    resource_type: &str,
) -> Vec<HeldResource> {
    match resource_type {
        RESOURCE_TYPE_EC2_VPC
        | RESOURCE_TYPE_EC2_SUBNET
        | RESOURCE_TYPE_EC2_SECURITY_GROUP
        | RESOURCE_TYPE_EC2_NETWORK_INTERFACE
        | RESOURCE_TYPE_EC2_INSTANCE
        | RESOURCE_TYPE_EC2_EIP => ec2_resources(engine, region, resource_type),
        RESOURCE_TYPE_ELBV2_LOAD_BALANCER => load_balancers_v2(engine),
        RESOURCE_TYPE_ELB_LOAD_BALANCER => load_balancers_classic(engine, region),
        RESOURCE_TYPE_CLOUDFRONT_DISTRIBUTION => distributions(engine),
        RESOURCE_TYPE_APIGATEWAY_STAGE => api_gateway_stages(engine, region),
        RESOURCE_TYPE_NETWORK_FIREWALL_FIREWALL => firewalls(engine),
        _ => Vec::new(),
    }
}

/// The organisation's resources of `resource_type` that `account` owns in
/// `region` and that are available to be associated with a resource set, keyed
/// by URI — the total order that makes a `ListDiscoveredResources` page
/// resumable from a key.
///
/// This is a pure READ of the stores of the services that own the resources: a
/// resource created through one of those services is discoverable with no
/// propagation step, and one deleted there stops being discovered, because
/// nothing is copied here. An account that owns none, and a type no emulated
/// service holds, both answer an EMPTY inventory — never an invented resource.
pub fn discovered_resources<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    account: &str,
    resource_type: &str,
) -> BTreeMap<String, DiscoveredResource> {
    let mut inventory = BTreeMap::new();
    for held in held_resources(engine, region, resource_type) {
        if held.account != account || !in_region(&held.uri, region) {
            continue;
        }
        let name = is_resource_name(&held.name).then(|| held.name.clone());
        inventory.insert(
            held.uri.clone(),
            DiscoveredResource {
                uri: Some(held.uri),
                account_id: Some(held.account),
                r#type: Some(resource_type.to_string()),
                name,
            },
        );
    }
    inventory
}

/// The accounts the discoverable inventory reports resources in, in `region`:
/// by construction, the accounts Firewall Manager discovered resources in, and
/// so accounts of the organisation whose resources a resource set may hold.
///
/// Derived from the SAME reads as [`discovered_resources`], so an account a
/// resource is discoverable in is one `BatchAssociateResource` accepts that
/// resource from, and the two cannot disagree.
pub fn discoverable_accounts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
) -> BTreeSet<String> {
    DISCOVERABLE_RESOURCE_TYPES
        .iter()
        .flat_map(|resource_type| held_resources(engine, region, resource_type))
        .filter(|held| in_region(&held.uri, region))
        .map(|held| held.account)
        .collect()
}

/// The mint behind [`mint_identifier`] and [`mint_base62_id`]: `len` lowercase
/// hex characters of the SHA-256 of a process-wide counter and the clock.
fn mint_hex(len: usize) -> String {
    use ::sha2::Digest;

    /// Distinguishes mints within one process; `Relaxed` is enough because only
    /// the uniqueness of the returned value matters, not the order.
    static MINTED: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0_u128, |since_epoch| since_epoch.as_nanos());
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(counter.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    let mut id = ::hex::encode(hasher.finalize());
    // SHA-256 renders as 64 hex characters, so every modelled length asked for
    // here is always there.
    id.truncate(len);
    id
}
