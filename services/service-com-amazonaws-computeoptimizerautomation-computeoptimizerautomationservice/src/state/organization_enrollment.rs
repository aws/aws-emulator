// SPDX-License-Identifier: CC0-1.0

//! The ORGANIZATION ENROLLMENT CONFIGURATIONS this service stores, and the
//! effective organization rule mode they supply.
//!
//! An organization enrollment configuration is stored on ONE target — a member
//! account, an organizational unit (OU) or the organization root — and it
//! supplies the organization rule mode of every account that target covers:
//! "The organization enrollment configuration sets the organization rule mode on
//! the specified account, organizational unit (OU), or organization root and
//! enables Automation for affected member accounts"
//! (`smithy_query(query="doc",shape="PutOrganizationEnrollmentConfiguration")`),
//! and a target the configuration does not sit on directly "inherit[s] the
//! organization rule mode from the closest parent that has an organization
//! enrollment configuration, or get[s] None allowed if no parent has one"
//! (`smithy_query(query="doc",shape="DeleteOrganizationEnrollmentConfiguration")`).
//!
//! The hierarchy those parents come from is NOT this service's: the organization
//! root, the OUs under it and which node each account sits under are records the
//! Organizations control plane owns, so [`OrganizationHierarchy`] reads them from
//! that service's own store — the same store `organizations:ListRoots` and
//! `organizations:ListOrganizationalUnitsForParent` answer from — rather than
//! this service keeping a second copy of the organization. An account in no
//! organization therefore has no root and no OU, which is what an empty
//! hierarchy says.

use std::collections::BTreeMap;

use service_com_amazonaws_organizations_awsorganizationsv20161128::state as organizations_state;

use super::{
    canonical_membership, ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState,
    ORGANIZATION_RULE_MODE_NONE_ALLOWED,
};
use crate::ops::types::{
    OrganizationEnrollmentConfiguration, OrganizationRuleModeSource, OrganizationTarget,
    OrganizationalUnitInfo,
};

/// The `OrganizationTargetType` of a target named by its 12-digit account id.
///
/// `OrganizationTargetType` is `Account | OrganizationalUnit`
/// (`smithy_query(query="shape",shape="OrganizationTargetType")`), and
/// `OrganizationTarget` is the union of exactly those two ways of naming a
/// target (`accountId` / `organizationalUnitId`), so the type is read off which
/// member the request sent ([`target_type_of`]).
pub const ORGANIZATION_TARGET_TYPE_ACCOUNT: &str = "Account";

/// The `OrganizationTargetType` of a target named by an OU or organization-root
/// id. The organization root is reported under this type too: the filter member
/// says so — "If you specify `OrganizationalUnit`, the results also include
/// configurations stored on the organization root"
/// (`smithy_query(query="doc",shape="ListOrganizationEnrollmentConfigurationsRequest",member="targetType")`)
/// — and the model gives the root no type of its own.
pub const ORGANIZATION_TARGET_TYPE_ORGANIZATIONAL_UNIT: &str = "OrganizationalUnit";

/// The one `OrganizationEnrollmentConfigurationType` the model declares
/// (`smithy_query(query="shape",shape="OrganizationEnrollmentConfigurationType")`):
/// the organization rule mode. It is what a delete names to remove and what a
/// put stores.
pub const ORGANIZATION_ENROLLMENT_CONFIGURATION_TYPE_RULE_MODE: &str = "OrganizationRuleMode";

/// `AutoEnrollMemberAccounts` for a root or OU an organization enrollment
/// configuration APPLIES to: "An organization enrollment configuration applies
/// and enables Automation for affected member accounts, including accounts that
/// join later. This value applies to configurations with either `AnyAllowed` or
/// `NoneAllowed`"
/// (`smithy_query(query="doc",shape="OrganizationalUnitInfo",member="autoEnrollMemberAccounts")`).
pub const AUTO_ENROLL_MEMBER_ACCOUNTS_ALL: &str = "All";

/// `AutoEnrollMemberAccounts` for a root or OU no configuration applies to: "No
/// organization enrollment configuration applies, so Automation isn't enabled by
/// an organization enrollment configuration" (same member's documentation).
pub const AUTO_ENROLL_MEMBER_ACCOUNTS_NONE: &str = "None";

/// `OrganizationRuleModeSourceType` for a target that carries the configuration
/// itself: "An organization enrollment configuration is on this organizational
/// unit or account"
/// (`smithy_query(query="doc",shape="OrganizationRuleModeSource",member="sourceType")`).
pub const RULE_MODE_SOURCE_APPLIED: &str = "Applied";

/// `OrganizationRuleModeSourceType` for a target that takes its mode from above:
/// "The organization enrollment configuration is on the parent organizational
/// unit or root" (same member's documentation).
pub const RULE_MODE_SOURCE_INHERITED: &str = "Inherited";

/// `OrganizationRuleModeSourceType` for an account whose mode was set on the
/// account itself: "The organization rule mode is set on the account itself,
/// with no organization enrollment configuration applied" (same member's
/// documentation) — which in this service is what an association writes.
pub const RULE_MODE_SOURCE_DIRECT: &str = "Direct";

/// `OrganizationRuleModeSourceType` for a target nothing has set a mode for:
/// "No organization enrollment configuration applies, so the organization rule
/// mode is None allowed" (same member's documentation).
pub const RULE_MODE_SOURCE_DEFAULT: &str = "Default";

/// The `OrganizationEnrollmentConfigurationStatus` of a member account an
/// organization enrollment configuration enables Automation for
/// (`smithy_query(query="shape",shape="OrganizationEnrollmentConfigurationStatus")`
/// is `Active | Inactive`).
pub const ORGANIZATION_ENROLLMENT_CONFIGURATION_STATUS_ACTIVE: &str = "Active";

/// The other `OrganizationEnrollmentConfigurationStatus`: no organization
/// enrollment configuration on the account or inherited from a parent enables
/// Automation for it.
pub const ORGANIZATION_ENROLLMENT_CONFIGURATION_STATUS_INACTIVE: &str = "Inactive";

/// How many organization enrollment configurations the store holds before a put
/// that would add another is refused.
///
/// CHOSEN bound, for the same reason [`super::MAX_ENROLLED_ACCOUNTS`] is: a
/// configuration is keyed by the target id the REQUEST supplies (any value
/// matching `AccountId` or `OrganizationalUnitId`), and this store lives in
/// memory, so the number of targets it holds is capped rather than left to grow
/// with every distinct id a caller names. A target already stored is written
/// over freely, because that adds nothing.
pub const MAX_ORGANIZATION_ENROLLMENT_CONFIGURATIONS: usize = 1_000;

/// One stored organization enrollment configuration. Every read path builds its
/// answer from one of these, never from the request that wrote it.
#[derive(Debug, Clone)]
pub struct StoredOrganizationEnrollmentConfiguration {
    /// The target type this configuration's id names
    /// ([`ORGANIZATION_TARGET_TYPE_ACCOUNT`] /
    /// [`ORGANIZATION_TARGET_TYPE_ORGANIZATIONAL_UNIT`]), read off which member
    /// of the `OrganizationTarget` union the put carried.
    pub target_type: String,
    /// The `OrganizationRuleMode` this configuration sets on its target.
    pub organization_rule_mode: String,
    /// When the configuration was last written. Kept because a configuration is
    /// always one some call stored, and that call records when.
    pub last_updated_timestamp: ::aws_smithy_types::DateTime,
}

/// The target id a request names, whichever member of the `OrganizationTarget`
/// union it sent.
pub fn target_id_of(target: &OrganizationTarget) -> &str {
    match target {
        OrganizationTarget::accountId(account_id) => account_id,
        OrganizationTarget::organizationalUnitId(organizational_unit_id) => organizational_unit_id,
    }
}

/// The `OrganizationTargetType` a request's target is of, read off which member
/// of the union it sent rather than off the shape of the id: the union is what
/// the model types the two kinds of target by.
pub fn target_type_of(target: &OrganizationTarget) -> &'static str {
    match target {
        OrganizationTarget::accountId(_) => ORGANIZATION_TARGET_TYPE_ACCOUNT,
        OrganizationTarget::organizationalUnitId(_) => ORGANIZATION_TARGET_TYPE_ORGANIZATIONAL_UNIT,
    }
}

/// The effective organization rule mode of one target, and where it came from.
///
/// The two always travel together: a mode is only meaningful beside the target
/// that supplied it, which is exactly what `OrganizationRuleModeSource` reports.
#[derive(Debug, Clone)]
pub struct EffectiveRuleMode {
    /// The `OrganizationRuleMode` in effect for the target.
    pub mode: String,
    /// How that mode is supplied, and the id of the target carrying the
    /// configuration it came from.
    pub source: OrganizationRuleModeSource,
}

impl EffectiveRuleMode {
    /// Is Automation enabled for the accounts this mode covers BY an
    /// organization enrollment configuration?
    ///
    /// True for exactly the two source types a configuration supplies — the one
    /// on the target and the one inherited from a parent — because a
    /// configuration "enables Automation for affected member accounts ...
    /// whether `AnyAllowed` or `NoneAllowed`", while a mode set on the account
    /// itself, or defaulted, is no configuration at all.
    pub fn enabled_by_configuration(&self) -> bool {
        self.source.source_type == RULE_MODE_SOURCE_APPLIED
            || self.source.source_type == RULE_MODE_SOURCE_INHERITED
    }
}

/// The organization this emulator's account is in, as the Organizations control
/// plane holds it: a SNAPSHOT, taken once per call so the lock on that service's
/// store is not held while this service's store is read.
///
/// Read from that service's own records rather than over the in-process
/// callback, because the hierarchy this resolution needs includes the node each
/// member ACCOUNT sits under, and the Organizations operations that report that
/// edge (`ListParents`, `ListAccountsForParent`) are not implemented yet, so an
/// API read would report every account as having no parent even where the
/// records say otherwise.
///
/// Empty (`root_id` and `management_account_id` absent, no OUs, no accounts)
/// while that store holds no organization, which is the state an account that
/// has never called `organizations:CreateOrganization` is in.
#[derive(Debug, Clone, Default)]
pub struct OrganizationHierarchy {
    /// The organization root's id, or `None` while this account is in no
    /// organization.
    pub root_id: Option<String>,
    /// The organization's management account id — its master account — or
    /// `None` while there is no organization.
    pub management_account_id: Option<String>,
    /// Every OU id -> the root id or OU id it hangs under, in ascending OU id
    /// order.
    pub organizational_unit_parents: BTreeMap<String, String>,
    /// Every member account id -> the root id or OU id it sits under, in
    /// ascending account id order.
    pub account_parents: BTreeMap<String, String>,
}

impl OrganizationHierarchy {
    /// Read the hierarchy out of the Organizations store reachable from
    /// `engine`. A READ: it stores nothing, and an engine that bears no such
    /// store answers the empty hierarchy.
    pub fn read<Eng: Send + Sync + 'static>(engine: &Eng) -> Self {
        let Some(organizations) = organizations_state::service_state(engine) else {
            return Self::default();
        };
        let guard = organizations.lock();
        let Some(organization) = guard.organization.as_ref() else {
            return Self::default();
        };
        let mut hierarchy = Self {
            root_id: guard.root().map(|root| root.id.clone()),
            management_account_id: Some(organization.master_account_id.clone()),
            ..Self::default()
        };
        for (id, record) in &guard.organizational_units {
            hierarchy
                .organizational_unit_parents
                .insert(id.clone(), record.parent_id.clone());
        }
        for (parent_id, children) in &guard.children {
            for child in children {
                if child.child_type == organizations_state::CHILD_TYPE_ACCOUNT {
                    hierarchy
                        .account_parents
                        .insert(child.id.clone(), parent_id.clone());
                }
            }
        }
        hierarchy
    }

    /// The organization root and every OU, root first and the OUs in ascending
    /// id order — the one stable sequence `ListOrganizationalUnits` pages over.
    ///
    /// The root leads because it is the parent of everything else, and the OUs
    /// follow in id order so that the pages of one walk are windows onto ONE
    /// sequence rather than samples of a shifting one.
    pub fn nodes_in_order(&self) -> Vec<String> {
        self.root_id
            .iter()
            .cloned()
            .chain(self.organizational_unit_parents.keys().cloned())
            .collect()
    }

    /// Does this hierarchy hold `id` as the root or as one of its OUs?
    pub fn holds_node(&self, id: &str) -> bool {
        self.root_id.as_deref() == Some(id) || self.organizational_unit_parents.contains_key(id)
    }

    /// Is `account_id` a member account of this organization?
    pub fn holds_account(&self, account_id: &str) -> bool {
        self.account_parents.contains_key(account_id)
    }

    /// Is `account_id` this organization's management account?
    pub fn is_management_account(&self, account_id: &str) -> bool {
        self.management_account_id.as_deref() == Some(account_id)
    }

    /// The nodes above `id`, CLOSEST PARENT FIRST and ending at the root: the
    /// order "the closest parent that has an organization enrollment
    /// configuration" is looked for in.
    ///
    /// `id` may name an account, an OU or the root; the root has no parent, and
    /// an id this hierarchy does not hold has no ancestors either. Bounded by
    /// the number of OUs the hierarchy holds, so a record that ever named a
    /// parent outside it still terminates.
    pub fn ancestors_of(&self, id: &str) -> Vec<String> {
        let mut ancestors: Vec<String> = Vec::new();
        let mut cursor = self
            .account_parents
            .get(id)
            .or_else(|| self.organizational_unit_parents.get(id))
            .cloned();
        while let Some(parent) = cursor {
            if ancestors.iter().any(|seen| seen == &parent)
                || ancestors.len() > self.organizational_unit_parents.len()
            {
                break;
            }
            cursor = self.organizational_unit_parents.get(&parent).cloned();
            ancestors.push(parent);
        }
        ancestors
    }

    /// Is `account_id` in any of the OUs or organization roots `node_ids` names,
    /// or in a CHILD OU of one of them?
    ///
    /// The reach every member that names OUs has: "The rule applies to accounts
    /// in the specified OUs and their child OUs"
    /// (`smithy_query(query="doc",shape="OrganizationConfiguration",member="organizationalUnitIds")`),
    /// and the listings that filter by OU say the same. An account this
    /// hierarchy does not hold sits in no OU, so no id reaches it.
    pub fn account_is_under_any(&self, account_id: &str, node_ids: &[String]) -> bool {
        self.ancestors_of(account_id)
            .iter()
            .any(|ancestor| node_ids.iter().any(|node_id| node_id == ancestor))
    }
}

/// The accounts an ORGANIZATION-WIDE selection covers, as its four scope members
/// state it: the accounts named directly and the ones reached through an
/// included OU, less the accounts and OUs excluded.
///
/// One type for both selections that carry those members — the organization
/// configuration of a rule and the organization scope of a preview, which are
/// the same four members under the same documented rules — so a rule and the
/// preview of that rule cannot disagree about which accounts it covers.
#[derive(Debug, Clone, Default)]
pub struct AccountScope {
    /// "List of Amazon Web Services account IDs to include", or `None` when the
    /// selection names none.
    pub account_ids: Option<Vec<String>>,
    /// "The organizational unit (OU) or organization root IDs to include", or
    /// `None` when the selection names none.
    pub organizational_unit_ids: Option<Vec<String>>,
    /// "The Amazon Web Services account IDs to exclude".
    pub excluded_account_ids: Vec<String>,
    /// "The organizational unit (OU) or organization root IDs to exclude".
    pub excluded_organizational_unit_ids: Vec<String>,
}

impl AccountScope {
    /// The scope that covers exactly `account_ids` and nothing else — what a
    /// rule type scoped to one account selects.
    pub fn of_accounts(account_ids: Vec<String>) -> Self {
        Self {
            account_ids: Some(account_ids),
            ..Self::default()
        }
    }

    /// Does this scope name no account and no OU at all, so that it covers the
    /// whole organization?
    ///
    /// A selection with no inclusion names no SPECIFIC target, which is how a
    /// preview without an `organizationScope` is scoped and what an
    /// organization rule carrying no inclusion applies to.
    pub fn includes_everything(&self) -> bool {
        self.account_ids.is_none() && self.organizational_unit_ids.is_none()
    }

    /// Does this scope cover `account_id`?
    ///
    /// Inclusion first — named directly, or reached through an included OU or
    /// organization root (or every account, when the scope names no inclusion)
    /// — then the exclusions, which take precedence: "Exclusions take precedence
    /// over inclusions. An account that is in an included OU and is also
    /// excluded is excluded." And the management account is not reached THROUGH
    /// the hierarchy: "The rule doesn't apply to the management account when the
    /// account is reached through an OU or the organization root", so an
    /// inclusion that only covers it by OU does not cover it, while one that
    /// names it directly does.
    pub fn covers(&self, hierarchy: &OrganizationHierarchy, account_id: &str) -> bool {
        let named_directly = self
            .account_ids
            .as_ref()
            .is_some_and(|ids| ids.iter().any(|id| id == account_id));
        let under_included_unit = self
            .organizational_unit_ids
            .as_ref()
            .is_some_and(|ids| hierarchy.account_is_under_any(account_id, ids));
        if !(self.includes_everything() || named_directly || under_included_unit) {
            return false;
        }
        if self
            .excluded_account_ids
            .iter()
            .any(|id| id == account_id)
            || hierarchy.account_is_under_any(account_id, &self.excluded_organizational_unit_ids)
        {
            return false;
        }
        !(under_included_unit && !named_directly && hierarchy.is_management_account(account_id))
    }

    /// The scope as the page token of a paginated selection digests it: every
    /// member read as the SET it tests membership of, so two spellings of one
    /// scope continue ONE walk.
    pub fn canonical(&self) -> Self {
        Self {
            account_ids: self.account_ids.as_deref().map(canonical_membership),
            organizational_unit_ids: self
                .organizational_unit_ids
                .as_deref()
                .map(canonical_membership),
            excluded_account_ids: canonical_membership(&self.excluded_account_ids),
            excluded_organizational_unit_ids: canonical_membership(
                &self.excluded_organizational_unit_ids,
            ),
        }
    }
}

/// Does a row belonging to `account_id` survive a listing's
/// `organizationalUnitIds` filter?
///
/// The filter is "The organizational unit (OU) or organization root IDs to use
/// to filter the results. The response includes recommended actions for
/// accounts in the specified OUs and their child OUs"
/// (`smithy_query(query="doc",shape="ListRecommendedActionsRequest",member="organizationalUnitIds")`),
/// and the three sibling listings that carry the member word it the same way.
/// A request naming none filters nothing. A row that records no account belongs
/// to no account, so no OU reaches it.
pub fn row_is_in_organizational_units(
    hierarchy: &OrganizationHierarchy,
    account_id: Option<&str>,
    organizational_unit_ids: Option<&[String]>,
) -> bool {
    let Some(node_ids) = organizational_unit_ids else {
        return true;
    };
    account_id.is_some_and(|account_id| hierarchy.account_is_under_any(account_id, node_ids))
}

impl ComAmazonawsComputeoptimizerautomationComputeoptimizerautomationserviceState {
    /// The organization enrollment configuration stored on `target_id`, or
    /// `None` when none is. A READ: it stores nothing.
    pub fn organization_enrollment_configuration(
        &self,
        target_id: &str,
    ) -> Option<&StoredOrganizationEnrollmentConfiguration> {
        self.organization_enrollment_configurations.get(target_id)
    }

    /// Would storing a configuration on `target_id` leave the store holding more
    /// than [`MAX_ORGANIZATION_ENROLLMENT_CONFIGURATIONS`] of them? A target the
    /// store already holds one for costs nothing.
    pub fn would_exceed_organization_configuration_limit(&self, target_id: &str) -> bool {
        if self
            .organization_enrollment_configurations
            .contains_key(target_id)
        {
            return false;
        }
        self.organization_enrollment_configurations.len() + 1
            > MAX_ORGANIZATION_ENROLLMENT_CONFIGURATIONS
    }

    /// Store the organization rule mode `mode` on `target_id`, which is of
    /// `target_type`, as `PutOrganizationEnrollmentConfiguration` documents:
    /// the call "Creates or updates an organization enrollment configuration".
    ///
    /// The configuration "take[s] precedence over settings made by
    /// `AssociateAccounts` and `DisassociateAccounts`", and deleting it later
    /// does not bring those settings back — "earlier account associations aren't
    /// restored"
    /// (`smithy_query(query="doc",shape="DeleteOrganizationEnrollmentConfiguration")`)
    /// — so the rule mode an association set on each account this target covers
    /// is DROPPED here rather than shadowed: from now on the mode of those
    /// accounts comes from the configuration, and removing the configuration
    /// leaves them with no mode of their own. Each account's enrollment STATUS is
    /// untouched, which is what "Automation stays enabled for affected accounts"
    /// after a delete means.
    ///
    /// Answers whether the stored configuration CHANGED. `at` is stamped only
    /// when it did, so a put that repeats the configuration the target already
    /// carries leaves it, timestamp included, exactly where it was.
    pub fn put_organization_enrollment_configuration(
        &mut self,
        hierarchy: &OrganizationHierarchy,
        target_id: &str,
        target_type: &str,
        mode: &str,
        at: ::aws_smithy_types::DateTime,
    ) -> bool {
        for account_id in self.accounts_covered_by(hierarchy, target_id) {
            if let Some(enrollment) = self.enrollments.get_mut(&account_id) {
                if enrollment.organization_rule_mode.take().is_some() {
                    enrollment.last_updated_timestamp = Some(at);
                }
            }
        }
        if let Some(stored) = self.organization_enrollment_configurations.get(target_id) {
            if stored.target_type == target_type && stored.organization_rule_mode == mode {
                return false;
            }
        }
        self.organization_enrollment_configurations.insert(
            target_id.to_string(),
            StoredOrganizationEnrollmentConfiguration {
                target_type: target_type.to_string(),
                organization_rule_mode: mode.to_string(),
                last_updated_timestamp: at,
            },
        );
        true
    }

    /// Remove the configuration types `types` names from `target_id`'s
    /// organization enrollment configuration, as
    /// `DeleteOrganizationEnrollmentConfiguration` documents: it "Deletes the
    /// specified configuration types from an organization enrollment
    /// configuration".
    ///
    /// The model declares exactly one configuration type
    /// ([`ORGANIZATION_ENROLLMENT_CONFIGURATION_TYPE_RULE_MODE`]) and a
    /// configuration holds nothing besides it, so removing that type removes the
    /// whole stored configuration — and the target then inherits its mode from
    /// the closest parent that has one, or gets None allowed.
    ///
    /// Answers whether the store CHANGED: a target that carries no
    /// configuration, or a request naming no type this store holds, removes
    /// nothing.
    pub fn delete_organization_enrollment_configuration(
        &mut self,
        target_id: &str,
        types: &[String],
    ) -> bool {
        if !types
            .iter()
            .any(|kind| kind == ORGANIZATION_ENROLLMENT_CONFIGURATION_TYPE_RULE_MODE)
        {
            return false;
        }
        self.organization_enrollment_configurations
            .remove(target_id)
            .is_some()
    }

    /// Every stored organization enrollment configuration `target_type` admits,
    /// projected onto the modelled shape, in ascending target-id order.
    ///
    /// `target_type` is the listing's filter: `Account` selects the
    /// configurations stored on accounts and `OrganizationalUnit` those stored
    /// on OUs and on the organization root, while `None` — a request that names
    /// no target type — "returns configurations for every target type".
    ///
    /// The ORDER is imposed by the store's own key order, so it is the same
    /// sequence on every call, which is what paging over it requires. A READ: it
    /// stores nothing.
    pub fn organization_enrollment_configurations_in_order(
        &self,
        target_type: Option<&str>,
    ) -> Vec<OrganizationEnrollmentConfiguration> {
        self.organization_enrollment_configurations
            .iter()
            .filter(|(_, stored)| target_type.is_none_or(|wanted| wanted == stored.target_type))
            .map(
                |(target_id, stored)| OrganizationEnrollmentConfiguration {
                    target_id: target_id.clone(),
                    target_type: stored.target_type.clone(),
                    organization_rule_mode: Some(stored.organization_rule_mode.clone()),
                },
            )
            .collect()
    }

    /// The effective organization rule mode of the root or OU `node_id`, and
    /// where it comes from: the configuration stored on the node itself
    /// (`Applied`), else the one on the closest parent that has one
    /// (`Inherited`), else None allowed (`Default`).
    pub fn effective_rule_mode_for_node(
        &self,
        hierarchy: &OrganizationHierarchy,
        node_id: &str,
    ) -> EffectiveRuleMode {
        if let Some(stored) = self.organization_enrollment_configuration(node_id) {
            return EffectiveRuleMode {
                mode: stored.organization_rule_mode.clone(),
                source: OrganizationRuleModeSource {
                    source_type: RULE_MODE_SOURCE_APPLIED.to_string(),
                    source_target_id: Some(node_id.to_string()),
                },
            };
        }
        self.inherited_rule_mode(hierarchy, node_id)
            .unwrap_or_else(default_rule_mode)
    }

    /// The effective organization rule mode of the ACCOUNT `account_id`, and
    /// where it comes from.
    ///
    /// The same resolution as for a root or an OU, with the one source type only
    /// an account has between the inherited configuration and the default: a
    /// mode set on the account itself (`Direct`), which in this service is what
    /// `AssociateAccounts` and `DisassociateAccounts` write. Configurations come
    /// FIRST because they "take precedence over settings made by
    /// `AssociateAccounts` and `DisassociateAccounts`"
    /// (`smithy_query(query="doc",shape="PutOrganizationEnrollmentConfiguration")`).
    pub fn effective_rule_mode_for_account(
        &self,
        hierarchy: &OrganizationHierarchy,
        account_id: &str,
    ) -> EffectiveRuleMode {
        if let Some(stored) = self.organization_enrollment_configuration(account_id) {
            return EffectiveRuleMode {
                mode: stored.organization_rule_mode.clone(),
                source: OrganizationRuleModeSource {
                    source_type: RULE_MODE_SOURCE_APPLIED.to_string(),
                    source_target_id: Some(account_id.to_string()),
                },
            };
        }
        if let Some(inherited) = self.inherited_rule_mode(hierarchy, account_id) {
            return inherited;
        }
        match self.enrollment(account_id).organization_rule_mode {
            Some(mode) => EffectiveRuleMode {
                mode,
                source: OrganizationRuleModeSource {
                    source_type: RULE_MODE_SOURCE_DIRECT.to_string(),
                    // Omitted for `Direct`: the mode sits on the account itself,
                    // so there is no other target to name
                    // (`smithy_query(query="doc",shape="OrganizationRuleModeSource",member="sourceTargetId")`).
                    source_target_id: None,
                },
            },
            None => default_rule_mode(),
        }
    }

    /// The accounts a configuration on `target_id` COVERS, among the accounts
    /// this service holds an enrollment for: the target itself when it names one
    /// of them, and every account the organization places at or under the target
    /// when it names a root or an OU.
    ///
    /// A configuration "sets the organization rule mode on the specified
    /// account, organizational unit (OU), or organization root and enables
    /// Automation for affected member accounts", so these are the accounts
    /// affected by it.
    fn accounts_covered_by(
        &self,
        hierarchy: &OrganizationHierarchy,
        target_id: &str,
    ) -> Vec<String> {
        self.enrollments
            .keys()
            .filter(|account_id| {
                account_id.as_str() == target_id
                    || hierarchy
                        .ancestors_of(account_id)
                        .iter()
                        .any(|ancestor| ancestor == target_id)
            })
            .cloned()
            .collect()
    }

    /// The mode `id` takes from the closest parent that carries a configuration,
    /// or `None` when no parent does.
    fn inherited_rule_mode(
        &self,
        hierarchy: &OrganizationHierarchy,
        id: &str,
    ) -> Option<EffectiveRuleMode> {
        hierarchy.ancestors_of(id).into_iter().find_map(|ancestor| {
            let stored = self.organization_enrollment_configuration(&ancestor)?;
            Some(EffectiveRuleMode {
                mode: stored.organization_rule_mode.clone(),
                source: OrganizationRuleModeSource {
                    source_type: RULE_MODE_SOURCE_INHERITED.to_string(),
                    source_target_id: Some(ancestor),
                },
            })
        })
    }

    /// The organization root and the OUs `wanted` names, each as the
    /// `OrganizationalUnitInfo` `ListOrganizationalUnits` reports, in the
    /// hierarchy's own stable order.
    ///
    /// `wanted` is the request's `organizationalUnitIds`: "If you don't specify
    /// this parameter, the operation returns the organization root and every OU"
    /// (`smithy_query(query="doc",shape="ListOrganizationalUnitsRequest",member="organizationalUnitIds")`),
    /// so `None` selects every node. An id the organization does not hold is
    /// reported by nothing: the listing answers from the hierarchy, not from the
    /// request.
    ///
    /// A READ: it stores nothing.
    pub fn organizational_units_in_order(
        &self,
        hierarchy: &OrganizationHierarchy,
        wanted: Option<&[String]>,
    ) -> Vec<OrganizationalUnitInfo> {
        hierarchy
            .nodes_in_order()
            .into_iter()
            .filter(|node_id| wanted.is_none_or(|ids| ids.iter().any(|id| id == node_id)))
            .map(|node_id| {
                let effective = self.effective_rule_mode_for_node(hierarchy, &node_id);
                OrganizationalUnitInfo {
                    organizational_unit_id: node_id,
                    auto_enroll_member_accounts: if effective.enabled_by_configuration() {
                        AUTO_ENROLL_MEMBER_ACCOUNTS_ALL.to_string()
                    } else {
                        AUTO_ENROLL_MEMBER_ACCOUNTS_NONE.to_string()
                    },
                    organization_rule_mode: effective.mode,
                    organization_rule_mode_source: effective.source,
                }
            })
            .collect()
    }

    /// The `organizationEnrollmentConfigurationStatus`
    /// `GetEnrollmentConfiguration` reports for `account_id`, or `None` when the
    /// member is omitted.
    ///
    /// The member "Specifies whether an organization enrollment configuration on
    /// your account, or inherited from a parent OU or the organization root,
    /// enables Automation for your member account. ... This field is returned
    /// for organization member accounts, including delegated administrators.
    /// It's omitted for management accounts and accounts that aren't in an
    /// organization"
    /// (`smithy_query(query="doc",shape="GetEnrollmentConfigurationResponse",member="organizationEnrollmentConfigurationStatus")`).
    /// So both halves are read from the organization itself: whether the account
    /// is a member of one and is not its management account decides whether the
    /// member is present at all, and whether a configuration applies to the
    /// account decides which of the two values it carries.
    pub fn organization_enrollment_configuration_status(
        &self,
        hierarchy: &OrganizationHierarchy,
        account_id: &str,
    ) -> Option<String> {
        if !hierarchy.holds_account(account_id) || hierarchy.is_management_account(account_id) {
            return None;
        }
        Some(
            if self
                .effective_rule_mode_for_account(hierarchy, account_id)
                .enabled_by_configuration()
            {
                ORGANIZATION_ENROLLMENT_CONFIGURATION_STATUS_ACTIVE.to_string()
            } else {
                ORGANIZATION_ENROLLMENT_CONFIGURATION_STATUS_INACTIVE.to_string()
            },
        )
    }
}

/// The effective mode of a target nothing has set one for: None allowed, from no
/// source at all.
fn default_rule_mode() -> EffectiveRuleMode {
    EffectiveRuleMode {
        mode: ORGANIZATION_RULE_MODE_NONE_ALLOWED.to_string(),
        source: OrganizationRuleModeSource {
            source_type: RULE_MODE_SOURCE_DEFAULT.to_string(),
            // Omitted for `Default`: no target carries a configuration to name.
            source_target_id: None,
        },
    }
}

/// The `InvalidParameterValueException` message for a put whose target is the
/// organization's management account.
///
/// States the rule the request broke — "Organization enrollment configurations
/// can't be created for the management account"
/// (`smithy_query(query="doc",shape="PutOrganizationEnrollmentConfiguration")`)
/// — and the target it named, which is the one value the caller has to replace.
pub fn management_account_target_message(target_id: &str) -> String {
    format!(
        "An organization enrollment configuration cannot be created for the management account \
         {target_id}."
    )
}

/// The `InvalidParameterValueException` message for a put that would leave the
/// store holding more than [`MAX_ORGANIZATION_ENROLLMENT_CONFIGURATIONS`]
/// configurations.
///
/// States the BOUND, which is the one fact a caller past it needs, and echoes no
/// target the request carried.
pub fn organization_configuration_limit_exceeded_message() -> String {
    format!(
        "The request would exceed the limit of {MAX_ORGANIZATION_ENROLLMENT_CONFIGURATIONS} \
         organization enrollment configurations"
    )
}
