// SPDX-License-Identifier: CC0-1.0

//! Deleting an IPAM: what the account still holds inside one, the move a delete leaves it in,
//! and the removal the clock owes it.
//!
//! "Delete an IPAM. Deleting an IPAM removes all monitored data associated with the IPAM"
//! (`DeleteIpam`): an IPAM is the container of its scopes, of the pools created in those
//! scopes and of the CIDRs provisioned to those pools, so the delete is the ONE place that
//! says what goes with it — and what has to be taken away first by a caller that did not ask
//! for a cascade.
//!
//! The record is left in the transient `delete-in-progress` state the `IpamState` catalog
//! carries and the clock owes it the REMOVAL one [`TRANSITION_DELAY`] later
//! ([`ComAmazonawsEc2Amazonec2State::ipam_delete_at`]), rather than a terminal state the
//! record keeps reporting: a deleted IPAM is one `DescribeIpams` answers
//! [`INVALID_IPAM_ID_NOT_FOUND`] for, so the row cannot stay in the store for ever.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// An IPAM whose deletion has been asked for and has not settled yet: the `IpamState` enum's
/// `delete-in-progress` member (`smithy_query(query="shape",shape="IpamState")`), which is
/// what `DeleteIpam` answers with.
pub const IPAM_STATE_DELETE_IN_PROGRESS: &str = "delete-in-progress";

/// Whether the IPAM `ipam` has had its deletion asked for already — the one definition of
/// "this IPAM is on its way out", so no path restarts the clock on one.
pub fn ipam_is_deleting(ipam: &Ipam) -> bool {
    ipam.state.as_deref() == Some(IPAM_STATE_DELETE_IN_PROGRESS)
}

/// An IPAM scope whose deletion has been asked for and has not settled yet: the
/// `IpamScopeState` enum's `delete-in-progress` member
/// (`smithy_query(query="shape",shape="IpamScopeState")`), which is what `DeleteIpamScope`
/// answers with.
pub const IPAM_SCOPE_STATE_DELETE_IN_PROGRESS: &str = "delete-in-progress";

/// Whether the scope `scope` has had its deletion asked for already — the one definition of
/// "this scope is on its way out", so no path restarts the clock on one and no reader counts
/// it among what its IPAM still holds.
pub fn ipam_scope_is_deleting(scope: &IpamScope) -> bool {
    scope.state.as_deref() == Some(IPAM_SCOPE_STATE_DELETE_IN_PROGRESS)
}

/// Whether the scope `scope` is one of the two an IPAM comes up with: "defines if the scope
/// is the default scope or not" (`IpamScope$IsDefault`).
///
/// The one definition of that question, so the delete that refuses a default scope and the
/// create that marks one agree about which scopes are default.
pub fn ipam_scope_is_default(scope: &IpamScope) -> bool {
    scope.is_default.unwrap_or(false)
}

impl ComAmazonawsEc2Amazonec2State {
    /// The `IpamScopeId`s of the scopes of the IPAM `ipam_id` names.
    ///
    /// A scope names its IPAM by `IpamArn` (the modelled [`IpamScope`] carries no IPAM id at
    /// all), which is what the scopes of this IPAM are found by — the same reading
    /// [`Self::recount_ipam_scopes`] counts them with.
    pub fn ipam_scope_ids_of_ipam(&self, ipam_id: &str) -> Vec<String> {
        let Some(ipam_arn) = self.ipams.get(ipam_id).and_then(|ipam| ipam.ipam_arn.clone()) else {
            return Vec::new();
        };
        self.ipam_scopes
            .iter()
            .filter(|(_, scope)| scope.ipam_arn.as_deref() == Some(ipam_arn.as_str()))
            .map(|(scope_id, _)| scope_id.clone())
            .collect()
    }

    /// The `IpamPoolId`s of the pools the scope `scope_id` names still HOLDS: the ones no
    /// delete has been asked for ([`ipam_pool_is_deleting`]), which is the same set
    /// [`Self::recount_ipam_scope_pools`] counts.
    pub fn ipam_pool_ids_in_scope(&self, scope_id: &str) -> Vec<String> {
        let Some(scope_arn) = self
            .ipam_scopes
            .get(scope_id)
            .and_then(|scope| scope.ipam_scope_arn.clone())
        else {
            return Vec::new();
        };
        self.ipam_pools
            .iter()
            .filter(|(_, pool)| {
                pool.ipam_scope_arn.as_deref() == Some(scope_arn.as_str())
                    && !ipam_pool_is_deleting(pool)
            })
            .map(|(pool_id, _)| pool_id.clone())
            .collect()
    }

    /// The `IpamPoolId`s of the pools of the IPAM `ipam_id` names that are in a scope of
    /// `scope_type`, through the two readings above — the pools a `DeleteIpam` has to decide
    /// about: "you cannot delete the IPAM with this option if there is a pool in your public
    /// scope" (`DeleteIpamRequest$Cascade`) is asked of the public ones, and the private ones
    /// are what the cascade deletes.
    pub fn ipam_pool_ids_in_ipam_scopes(&self, ipam_id: &str, scope_type: &str) -> Vec<String> {
        let mut pools: Vec<String> = Vec::new();
        for scope_id in self.ipam_scope_ids_of_ipam(ipam_id) {
            if self
                .ipam_scopes
                .get(&scope_id)
                .and_then(|scope| scope.ipam_scope_type.as_deref())
                != Some(scope_type)
            {
                continue;
            }
            pools.extend(self.ipam_pool_ids_in_scope(&scope_id));
        }
        pools.sort();
        pools
    }

    /// Start deleting the IPAM `ipam_id` names, and answer it as the store then holds it —
    /// `None` when the account holds no such IPAM.
    ///
    /// The IPAM is left in the TRANSIENT [`IPAM_STATE_DELETE_IN_PROGRESS`] state with the
    /// clock owing it the removal one [`TRANSITION_DELAY`] later
    /// ([`Self::ipam_delete_at`], applied by [`Self::materialize_due_transitions`]) — the
    /// same arrangement, on the same clock, as the `create-in-progress` the IPAM started in,
    /// so a caller that reads straight back still sees the delete under way and a later one
    /// finds the IPAM gone. The span is never zero and nothing is slept for.
    ///
    /// Any move the clock still owed this IPAM is superseded: an IPAM on its way out is not
    /// going to land in the state its creation or a modification was moving it to.
    pub fn begin_ipam_deletion(&mut self, ipam_id: &str) -> Option<Ipam> {
        let ipam = self.ipams.get_mut(ipam_id)?;
        ipam.state = Some(IPAM_STATE_DELETE_IN_PROGRESS.to_string());
        let view = ipam.clone();
        self.ipam_create_complete_at.remove(ipam_id);
        self.ipam_modify_complete_at.remove(ipam_id);
        self.ipam_delete_at
            .insert(ipam_id.to_string(), Instant::now() + TRANSITION_DELAY);
        Some(view)
    }

    /// Remove the IPAM `ipam_id` names with everything it contains, giving the bytes all of
    /// it held back to the store's budget.
    ///
    /// "Deleting an IPAM removes all monitored data associated with the IPAM" (`DeleteIpam`),
    /// and what the store holds inside one is its scopes, the pools created in them, the
    /// CIDRs provisioned to those pools ([`Self::remove_ipam_pool_cidrs`]) and the default
    /// resource discovery and association that came up with it — so all of it goes here,
    /// rather than leaving rows naming an IPAM, a scope or a pool nothing resolves. The
    /// `ClientToken`s that minted the records go with them, for the same reason: a replay of
    /// one would otherwise answer with an id the store no longer holds.
    pub(super) fn remove_ipam(&mut self, ipam_id: &str) {
        let Some(ipam) = self.ipams.remove(ipam_id) else {
            return;
        };
        // Exactly the share [`ipam_payload_bytes`] charged a create for the three records it
        // wrote — the IPAM, its resource discovery and that discovery's association — so the
        // refund matches the charge.
        self.release_stored_bytes(
            tag_list_bytes(ipam.tags.as_deref().unwrap_or_default())
                .saturating_add(ipam_modifiable_bytes(&ipam))
                .saturating_add(3 * STORED_RECORD_OVERHEAD_BYTES),
        );
        let ipam_arn = ipam.ipam_arn.clone().unwrap_or_default();
        let scope_ids: Vec<String> = self
            .ipam_scopes
            .iter()
            .filter(|(_, scope)| scope.ipam_arn.as_deref() == Some(ipam_arn.as_str()))
            .map(|(scope_id, _)| scope_id.clone())
            .collect();
        for scope_id in scope_ids {
            self.remove_ipam_scope(&scope_id);
        }
        if let Some(discovery_id) = ipam.default_resource_discovery_id.as_deref() {
            self.ipam_resource_discoveries.remove(discovery_id);
        }
        if let Some(association_id) = ipam.default_resource_discovery_association_id.as_deref() {
            self.ipam_resource_discovery_associations.remove(association_id);
        }
        // An association something else made against this IPAM names an IPAM the store no
        // longer holds, so it goes too.
        let associations: Vec<String> = self
            .ipam_resource_discovery_associations
            .iter()
            .filter(|(_, association)| association.ipam_id.as_deref() == Some(ipam_id))
            .map(|(association_id, _)| association_id.clone())
            .collect();
        for association_id in associations {
            self.ipam_resource_discovery_associations.remove(&association_id);
            self.ipam_resource_discovery_association_complete_at
                .remove(&association_id);
        }
        self.ipam_delete_at.remove(ipam_id);
        self.ipam_create_complete_at.remove(ipam_id);
        self.ipam_modify_complete_at.remove(ipam_id);
        self.release_client_tokens_naming(TokenHolder::Ipam, ipam_id);
    }

    /// Remove one IPAM scope with the pools created in it, giving the bytes they held back to
    /// the store's budget.
    ///
    /// A scope is "the highest-level container within IPAM" (`IpamScope`) a pool lives in, so
    /// a scope that leaves the store takes its pools — and, through
    /// [`Self::remove_ipam_pool`], the CIDRs provisioned to them — with it.
    pub(super) fn remove_ipam_scope(&mut self, scope_id: &str) {
        let Some(scope) = self.ipam_scopes.remove(scope_id) else {
            return;
        };
        self.release_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES
                .saturating_add(tag_list_bytes(scope.tags.as_deref().unwrap_or_default()))
                .saturating_add(scope.description.as_deref().map_or(0, str::len))
                .saturating_add(
                    scope
                        .external_authority_configuration
                        .as_ref()
                        .and_then(|configuration| {
                            configuration.external_resource_identifier.as_deref()
                        })
                        .map_or(0, str::len),
                ),
        );
        let scope_arn = scope.ipam_scope_arn.clone().unwrap_or_default();
        let pool_ids: Vec<String> = self
            .ipam_pools
            .iter()
            .filter(|(_, pool)| pool.ipam_scope_arn.as_deref() == Some(scope_arn.as_str()))
            .map(|(pool_id, _)| pool_id.clone())
            .collect();
        for pool_id in pool_ids {
            self.remove_ipam_pool(&pool_id);
        }
        self.ipam_scope_create_complete_at.remove(scope_id);
        self.ipam_scope_delete_at.remove(scope_id);
        self.release_client_tokens_naming(TokenHolder::IpamScope, scope_id);
    }

    /// Remove one IPAM pool with the CIDRs provisioned to it, giving the bytes they held back
    /// to the store's budget.
    ///
    /// The exact reverse of [`Self::insert_creating_ipam_pool`]: the record's tags, its
    /// description and the allocation resource tags it retained are given back, the CIDR
    /// records that named it go with it ([`Self::remove_ipam_pool_cidrs`]), and so does every
    /// move the clock still owed it.
    pub(super) fn remove_ipam_pool(&mut self, pool_id: &str) {
        let Some(pool) = self.ipam_pools.remove(pool_id) else {
            return;
        };
        self.release_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES
                .saturating_add(tag_list_bytes(pool.tags.as_deref().unwrap_or_default()))
                .saturating_add(pool.description.as_deref().map_or(0, str::len))
                .saturating_add(ipam_resource_tag_list_bytes(
                    pool.allocation_resource_tags.as_deref().unwrap_or_default(),
                )),
        );
        self.remove_ipam_pool_cidrs(pool_id);
        self.ipam_pool_create_complete_at.remove(pool_id);
        self.ipam_pool_delete_complete_at.remove(pool_id);
        self.release_client_tokens_naming(TokenHolder::IpamPool, pool_id);
    }

    /// Drop every `ClientToken` that names `resource_id` in one of the IPAM token maps, and
    /// give the bytes those tokens held back to the store's budget.
    ///
    /// A token left naming a record the store no longer holds would replay into a dangling
    /// id, so it goes with the record — the one place that rule lives for the IPAM family,
    /// which is why `holder` names the map rather than each caller walking it.
    fn release_client_tokens_naming(&mut self, holder: TokenHolder, resource_id: &str) {
        let tokens = match holder {
            TokenHolder::Ipam => &mut self.ipam_client_tokens,
            TokenHolder::IpamScope => &mut self.ipam_scope_client_tokens,
            TokenHolder::IpamPool => &mut self.ipam_pool_client_tokens,
        };
        let doomed: Vec<String> = tokens
            .iter()
            .filter(|(_, minted)| minted.as_str() == resource_id)
            .map(|(token, _)| token.clone())
            .collect();
        let mut released = 0usize;
        for token in doomed {
            tokens.remove(&token);
            released = released.saturating_add(token.len());
        }
        self.release_stored_bytes(released);
    }

    /// Apply the removal the clock owes a deleted IPAM, as of `now`.
    ///
    /// An IPAM a `DeleteIpam` left `delete-in-progress` was GONE at the instant that call
    /// recorded for it, whether or not anybody called again — so the removal is applied from
    /// [`Self::materialize_due_transitions`], its one caller, before anything reads the
    /// store.
    pub(super) fn materialize_due_ipam_deletions(&mut self, now: Instant) {
        let due: Vec<String> = self
            .ipam_delete_at
            .iter()
            .filter(|(_, due_at)| now >= **due_at)
            .map(|(ipam_id, _)| ipam_id.clone())
            .collect();
        for ipam_id in due {
            self.ipam_delete_at.remove(&ipam_id);
            self.remove_ipam(&ipam_id);
        }
    }

    /// The `IpamId` of the IPAM the scope `scope_id` belongs to, found by the `IpamArn` the
    /// scope names it by — the modelled [`IpamScope`] carries no IPAM id at all, the same
    /// reading [`Self::ipam_scope_id_of_pool`] has for a pool's scope.
    pub fn ipam_id_of_scope(&self, scope_id: &str) -> Option<String> {
        let ipam_arn = self.ipam_scopes.get(scope_id)?.ipam_arn.clone()?;
        self.ipams
            .iter()
            .find(|(_, ipam)| ipam.ipam_arn.as_deref() == Some(ipam_arn.as_str()))
            .map(|(ipam_id, _)| ipam_id.clone())
    }

    /// Start deleting the IPAM scope `scope_id` names, and answer it as the store then holds
    /// it — `None` when the account holds no such scope.
    ///
    /// The scope is left in the TRANSIENT [`IPAM_SCOPE_STATE_DELETE_IN_PROGRESS`] state the
    /// `IpamScopeState` catalog carries, with the clock owing its REMOVAL one
    /// [`TRANSITION_DELAY`] later ([`Self::ipam_scope_delete_at`], applied by
    /// [`Self::materialize_due_transitions`]) — the same arrangement, on the same clock, as
    /// the `create-in-progress` the scope started in. The span is never zero, so a caller
    /// acting straight after this call still sees the delete under way, and nothing is slept
    /// for.
    ///
    /// The record is REMOVED when that instant falls due rather than parked in a terminal
    /// state: a deleted scope is one `DescribeIpamScopes` answers
    /// [`INVALID_IPAM_SCOPE_ID_NOT_FOUND`] for, which is why this follows
    /// [`Self::begin_ipam_deletion`] rather than [`Self::begin_ipam_pool_deletion`]. The
    /// pools created in it go with it ([`Self::remove_ipam_scope`]), and the IPAM no longer
    /// HOLDS the scope, so its "number of scopes in the IPAM" is recounted exactly as a
    /// create recounts it.
    ///
    /// Any move the clock still owed this scope is superseded: a scope on its way out is not
    /// going to land in the state its creation was moving it to.
    pub fn begin_ipam_scope_deletion(&mut self, scope_id: &str) -> Option<IpamScope> {
        let ipam_id = self.ipam_id_of_scope(scope_id);
        let scope = self.ipam_scopes.get_mut(scope_id)?;
        scope.state = Some(IPAM_SCOPE_STATE_DELETE_IN_PROGRESS.to_string());
        let view = scope.clone();
        self.ipam_scope_create_complete_at.remove(scope_id);
        self.ipam_scope_delete_at
            .insert(scope_id.to_string(), Instant::now() + TRANSITION_DELAY);
        if let Some(ipam_id) = ipam_id {
            self.recount_ipam_scopes(&ipam_id);
        }
        Some(view)
    }

    /// Apply the removal the clock owes a deleted IPAM scope, as of `now`.
    ///
    /// A scope a `DeleteIpamScope` left `delete-in-progress` was GONE at the instant that
    /// call recorded for it, whether or not anybody called again — so the removal is applied
    /// from [`Self::materialize_due_transitions`], its one caller, before anything reads the
    /// store, exactly as [`Self::materialize_due_ipam_deletions`] applies an IPAM's.
    ///
    /// The IPAM's scope count does not CHANGE here: [`Self::begin_ipam_scope_deletion`]
    /// already recounted it without the scope on its way out. It is recounted again only so
    /// the count follows from the scopes the store holds at every point, whatever order the
    /// writers run in.
    pub(super) fn materialize_due_ipam_scope_deletions(&mut self, now: Instant) {
        let due: Vec<String> = self
            .ipam_scope_delete_at
            .iter()
            .filter(|(_, due_at)| now >= **due_at)
            .map(|(scope_id, _)| scope_id.clone())
            .collect();
        for scope_id in due {
            self.ipam_scope_delete_at.remove(&scope_id);
            let ipam_id = self.ipam_id_of_scope(&scope_id);
            self.remove_ipam_scope(&scope_id);
            if let Some(ipam_id) = ipam_id {
                self.recount_ipam_scopes(&ipam_id);
            }
        }
    }
}

/// Which IPAM-family `ClientToken` map [`ComAmazonawsEc2Amazonec2State::release_client_tokens_naming`]
/// is to drop entries from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenHolder {
    /// [`ComAmazonawsEc2Amazonec2State::ipam_client_tokens`].
    Ipam,
    /// [`ComAmazonawsEc2Amazonec2State::ipam_scope_client_tokens`].
    IpamScope,
    /// [`ComAmazonawsEc2Amazonec2State::ipam_pool_client_tokens`].
    IpamPool,
}
