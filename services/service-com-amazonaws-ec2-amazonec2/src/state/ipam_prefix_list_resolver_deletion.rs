// SPDX-License-Identifier: CC0-1.0

//! Deleting an IPAM prefix list resolver: what still has to be taken away first, the move a
//! delete leaves the resolver in, and the terminal state the clock owes it.
//!
//! "Deletes an IPAM prefix list resolver. Before deleting a resolver, you must first delete all
//! resolver targets associated with it" (`DeleteIpamPrefixListResolver`): a resolver's targets
//! are what it synchronizes prefix lists through, so a resolver that still has one cannot be
//! deleted — and the targets are the caller's to take away, because the operation documents no
//! cascade.
//!
//! The record is left in the transient `delete-in-progress` state the
//! `IpamPrefixListResolverState` catalog carries and the clock owes it `delete-complete` one
//! [`TRANSITION_DELAY`] later
//! ([`ComAmazonawsEc2Amazonec2State::ipam_prefix_list_resolver_delete_complete_at`]) — the same
//! arrangement, on the same clock, as the IPAM POOL's two delete states
//! ([`ComAmazonawsEc2Amazonec2State::begin_ipam_pool_deletion`]), because this resource's state
//! catalog carries that same pair and so a deleted resolver still has a state to report.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every other
//! primitive there.

use super::*;

/// A resolver whose deletion has been asked for and has not settled yet: the
/// `IpamPrefixListResolverState` enum's `delete-in-progress` member
/// (`smithy_query(query="shape",shape="IpamPrefixListResolverState")`), which is what
/// `DeleteIpamPrefixListResolver` answers with.
pub const IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_IN_PROGRESS: &str = "delete-in-progress";

/// The state that enum reaches once the deletion has settled: `delete-complete`, which the
/// clock owes a [`IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_IN_PROGRESS`] resolver one
/// [`TRANSITION_DELAY`] after the call that deleted it.
pub const IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_COMPLETE: &str = "delete-complete";

/// Whether `resolver` has had its deletion asked for already — either delete state counts, so
/// no path restarts the clock on a resolver that is on its way out or already gone.
///
/// The one definition of "this resolver is being deleted", read by the delete that refuses a
/// second one.
pub fn ipam_prefix_list_resolver_is_deleting(resolver: &IpamPrefixListResolver) -> bool {
    is_a_delete_state(resolver.state.as_deref())
}

/// Whether `target` has had its deletion asked for already, by the same reading: the
/// `IpamPrefixListResolverTargetState` catalog
/// (`smithy_query(query="shape",shape="IpamPrefixListResolverTargetState")`) carries the same
/// two delete members under the same spellings as the resolver's own.
///
/// What makes a target one its resolver still HOLDS, which is what
/// [`ComAmazonawsEc2Amazonec2State::ipam_prefix_list_resolver_target_ids_of_resolver`] reads.
pub fn ipam_prefix_list_resolver_target_is_deleting(
    target: &IpamPrefixListResolverTarget,
) -> bool {
    is_a_delete_state(target.state.as_deref())
}

/// Whether `state` is one of the two delete states both catalogs of this resource family
/// spell the same way.
fn is_a_delete_state(state: Option<&str>) -> bool {
    matches!(
        state,
        Some(IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_IN_PROGRESS)
            | Some(IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_COMPLETE)
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// The `IpamPrefixListResolverTargetId`s of the targets the resolver `resolver_id` names
    /// still HOLDS: the ones no delete has been asked for.
    ///
    /// A target names its resolver by `IpamPrefixListResolverTarget$IpamPrefixListResolverId`,
    /// which is what the targets of this resolver are found by — the same reading
    /// `DescribeIpamPrefixListResolverTargets` narrows by that member with.
    pub fn ipam_prefix_list_resolver_target_ids_of_resolver(
        &self,
        resolver_id: &str,
    ) -> Vec<String> {
        self.ipam_prefix_list_resolver_targets
            .iter()
            .filter(|(_, target)| {
                target.ipam_prefix_list_resolver_id.as_deref() == Some(resolver_id)
                    && !ipam_prefix_list_resolver_target_is_deleting(target)
            })
            .map(|(target_id, _)| target_id.clone())
            .collect()
    }

    /// Start deleting the IPAM prefix list resolver `resolver_id` names, and answer it as the
    /// store then holds it — `None` when the account holds no such resolver.
    ///
    /// The resolver is left in the TRANSIENT
    /// [`IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_IN_PROGRESS`] state with the clock owing it
    /// [`IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_COMPLETE`] one [`TRANSITION_DELAY`] later
    /// ([`Self::ipam_prefix_list_resolver_delete_complete_at`], applied by
    /// [`Self::materialize_due_transitions`]) — the same arrangement, on the same clock, as the
    /// `create-in-progress` the resolver started in, so a caller reading straight back still
    /// sees the delete under way. The span is never zero and nothing is slept for.
    ///
    /// Any move the clock still owed this resolver is superseded: a resolver on its way out is
    /// not going to land in the state its creation was moving it to.
    ///
    /// The record STAYS in the store, because a resolver in either delete state still has a
    /// state to report — so its caller-chosen bytes and the CIDR selection rules it was created
    /// with stay charged to the store's budget, which still holds them, exactly as
    /// [`Self::begin_ipam_pool_deletion`] keeps a deleted pool's.
    pub fn begin_ipam_prefix_list_resolver_deletion(
        &mut self,
        resolver_id: &str,
    ) -> Option<IpamPrefixListResolver> {
        let resolver = self.ipam_prefix_list_resolvers.get_mut(resolver_id)?;
        resolver.state = Some(IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_IN_PROGRESS.to_string());
        let view = resolver.clone();
        self.ipam_prefix_list_resolver_create_complete_at
            .remove(resolver_id);
        self.ipam_prefix_list_resolver_delete_complete_at
            .insert(resolver_id.to_string(), Instant::now() + TRANSITION_DELAY);
        Some(view)
    }

    /// Apply the move the clock owes a deleted resolver, as of `now`.
    ///
    /// A resolver a `DeleteIpamPrefixListResolver` left `delete-in-progress` was
    /// `delete-complete` at the instant that call recorded for it, whether or not anybody
    /// called again — so the move is applied from [`Self::materialize_due_transitions`], its one
    /// caller, before anything reads the store.
    pub(super) fn materialize_due_ipam_prefix_list_resolver_deletions(&mut self, now: Instant) {
        let resolvers = &mut self.ipam_prefix_list_resolvers;
        self.ipam_prefix_list_resolver_delete_complete_at
            .retain(|resolver_id, due_at| {
                if now < *due_at {
                    return true;
                }
                if let Some(resolver) = resolvers.get_mut(resolver_id) {
                    if resolver.state.as_deref()
                        == Some(IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_IN_PROGRESS)
                    {
                        resolver.state =
                            Some(IPAM_PREFIX_LIST_RESOLVER_STATE_DELETE_COMPLETE.to_string());
                    }
                }
                // The transition is applied once; nothing is owed on this resolver any more.
                false
            });
    }
}
