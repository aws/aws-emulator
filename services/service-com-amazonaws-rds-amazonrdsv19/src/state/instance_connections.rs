// SPDX-License-Identifier: CC0-1.0

//! How many client SESSIONS a DB resource is serving, counted the one way every reader of that
//! number counts them.
//!
//! Two readers inside this store ask it, and both mean the same thing by it — the sessions open on
//! the endpoints this emulator is really serving for that resource
//! ([`super::data_plane::pg_sessions::SessionRegistry`]):
//!
//! * the `DatabaseConnections` series each DB instance publishes
//!   ([`super::instance_metrics`]), whose datapoint IS that count; and
//! * the IDLENESS an Aurora Serverless v2 cluster's auto-pause is owed on
//!   ([`super::serverless_capacity`]), where a cluster serving any session is not idle.
//!
//! So the traversal lives here once rather than in each of them, and "the connections this resource
//! is serving" cannot come to mean two different things.

use super::{ComAmazonawsRdsAmazonrdsv19State as RdsState, RegionalKey};

/// How many sessions are open on the engine endpoint of the DB instance `key` names, and none at
/// all for an instance that has no endpoint in this process — an engine family this emulator
/// serves no wire for holds no session.
pub(super) fn instance_open_sessions(store: &RdsState, key: &RegionalKey) -> usize {
    store
        .engine_endpoint(key)
        .map_or(0, |endpoint| endpoint.sessions().open_count())
}

/// How many sessions `cluster` is serving: the ones on its OWN writer and reader endpoints, and
/// the ones on each member instance's own endpoint.
pub(super) fn cluster_open_sessions(store: &RdsState, cluster: &super::DbClusterRecord) -> usize {
    let key: RegionalKey = (cluster.region.clone(), cluster.identifier.clone());
    let own = store
        .cluster_engine_endpoint(&key)
        .map_or(0, |endpoint| endpoint.sessions().open_count());
    store
        .cluster_members(cluster)
        .into_iter()
        .map(|member| {
            instance_open_sessions(store, &(member.region.clone(), member.identifier.clone()))
        })
        .fold(own, usize::saturating_add)
}
