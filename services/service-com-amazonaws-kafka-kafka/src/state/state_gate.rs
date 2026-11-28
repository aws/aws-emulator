// SPDX-License-Identifier: CC0-1.0

//! The preconditions a call that CHANGES a resource of this service applies —
//! the version it is accepted FROM and the state it is accepted IN — and the one
//! sentence Amazon MSK refuses such a call with when the resource is not in a
//! state that call is accepted from.
//!
//! The service answers that condition with ONE fixed sentence, which names the set
//! of states the call IS accepted from and nothing else — not the state the
//! resource happens to be in, and no value from the request. It is built here, from
//! the accepted-state set the handler gated on, so the gate a call applies and the
//! sentence it answers with cannot drift apart, and so the cluster family and the
//! configuration family say the same thing about the same condition.

use super::*;

/// The `ClusterState` values a call that CHANGES a cluster's configuration is
/// accepted from.
///
/// A cluster reconfigures one change at a time, so an update is only ever applied
/// to a cluster that is serving and settled: one still being provisioned, already
/// applying a change, rebooting its brokers or being deleted has no settled
/// configuration to update from. Named here so every update operation gates on one
/// set and reports the one refusal built from it.
pub const CLUSTER_STATES_A_CHANGE_IS_ACCEPTED_FROM: [&str; 1] = [CLUSTER_STATE_ACTIVE];

/// Whether a cluster in `state` accepts a call that changes its configuration.
pub fn cluster_accepts_change(state: &str) -> bool {
    CLUSTER_STATES_A_CHANGE_IS_ACCEPTED_FROM.contains(&state)
}

/// The refusal every such call answers for a cluster that is in some other state:
/// a `BadRequestException` (HTTP 400) carrying the fixed sentence over
/// [`CLUSTER_STATES_A_CHANGE_IS_ACCEPTED_FROM`] and naming the member the request
/// addressed the cluster by as the parameter at fault, by the one wire spelling of
/// it this service keeps ([`CLUSTER_ARN_MEMBER`]).
///
/// One envelope, so the state gate of one update operation cannot drift from
/// another's, and a caller can tell "wait for the cluster to settle" from a
/// reference the service cannot resolve at all
/// ([`RESOURCE_NOT_FOUND_MESSAGE`]).
pub fn cluster_not_in_accepted_state() -> BadRequestException {
    BadRequestException {
        invalid_parameter: Some(CLUSTER_ARN_MEMBER.to_string()),
        message: Some(not_in_accepted_state_message(
            &CLUSTER_STATES_A_CHANGE_IS_ACCEPTED_FROM,
        )),
    }
}

/// The refusal a call that CHANGES a cluster answers when the version it quotes
/// is not the one the cluster will accept its next change from: a
/// `BadRequestException` (HTTP 400) carrying the service's one stale-version
/// sentence ([`CLUSTER_VERSION_NOT_CURRENT_MESSAGE`]) and naming the member the
/// version travelled in.
///
/// It tells the caller something different from the state gate above: read the
/// cluster's version again and retry with THAT, rather than retry this same
/// request once the cluster has settled. Built here beside the gate, because the
/// two conditions are decided together ([`cluster_change_refusal`]).
pub fn cluster_version_not_current() -> BadRequestException {
    BadRequestException {
        invalid_parameter: Some(CURRENT_VERSION_MEMBER.to_string()),
        message: Some(CLUSTER_VERSION_NOT_CURRENT_MESSAGE.to_string()),
    }
}

/// Why `cluster` does not accept the change a caller quoting `submitted_version`
/// asks for, or `None` when it does.
///
/// The two preconditions every such call applies, in the order the service
/// applies them: the version the caller is updating FROM is settled FIRST, so a
/// caller holding a version the cluster has moved past is told to read the
/// version again ([`cluster_version_not_current`]) rather than to wait — which is
/// what two callers racing from one version are told, since accepting the
/// winner's change generates the cluster's next version and leaves the loser
/// quoting a version the cluster is no longer at. Only a call quoting the version
/// the cluster IS at is answered on the state it is in
/// ([`cluster_not_in_accepted_state`]), which is the answer a caller that re-read
/// the cluster gets while its change rolls out.
///
/// One decision, so the two refusals of one condition cannot be ordered one way
/// by one update operation and the other way by the next.
pub fn cluster_change_refusal(
    submitted_version: &str,
    cluster: &Cluster,
) -> Option<BadRequestException> {
    if submitted_version != cluster.current_version {
        return Some(cluster_version_not_current());
    }
    if !cluster_accepts_change(&cluster.state) {
        return Some(cluster_not_in_accepted_state());
    }
    None
}

/// The one sentence that refusal carries: fixed text around the set of states the
/// call is accepted from, in the spelling the state enum declares, and nothing
/// else — so a gate that admits more than one state says so, and no refusal
/// echoes a value back.
pub fn not_in_accepted_state_message(accepted: &[&str]) -> String {
    format!(
        "This operation is only valid for resources that are in one of the following states :[{}]",
        accepted.join(", ")
    )
}

/// The `TopicState` values a call that CHANGES a topic's configuration is accepted
/// from.
///
/// A topic takes one configuration change at a time: one still being created, one
/// whose partitions are still being reassigned by a change already accepted, and
/// one whose deletion the service has accepted are none of them a topic a further
/// change applies to. The same rule
/// [`CLUSTER_STATES_A_CHANGE_IS_ACCEPTED_FROM`] states for a cluster, over the
/// topic's own state machine.
pub const TOPIC_STATES_A_CHANGE_IS_ACCEPTED_FROM: [&str; 1] = [TOPIC_STATE_ACTIVE];

/// Whether a topic in `state` accepts a call that changes its configuration.
pub fn topic_accepts_change(state: &str) -> bool {
    TOPIC_STATES_A_CHANGE_IS_ACCEPTED_FROM.contains(&state)
}

/// The refusal such a call answers for a topic that is in some other state, in the
/// same envelope the cluster gate answers: the fixed sentence over
/// [`TOPIC_STATES_A_CHANGE_IS_ACCEPTED_FROM`], naming the member the request
/// addressed the topic by ([`TOPIC_NAME_MEMBER`]).
pub fn topic_not_in_accepted_state() -> BadRequestException {
    BadRequestException {
        invalid_parameter: Some(TOPIC_NAME_MEMBER.to_string()),
        message: Some(not_in_accepted_state_message(
            &TOPIC_STATES_A_CHANGE_IS_ACCEPTED_FROM,
        )),
    }
}
