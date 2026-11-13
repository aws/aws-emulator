// SPDX-License-Identifier: CC0-1.0

//! THE REPLICATION ITSELF: the committed changes of a source DB instance applied to the
//! storage of every read replica of it.
//!
//! A read replica is "a read-only copy of your DB instance", kept up to date because "RDS
//! then uses the asynchronous replication method for the DB engine to update the read
//! replica" — so a client that writes the source and then reads the replica reads what it
//! wrote, and that is the whole point of pointing readers at a replica.
//!
//! What makes this emulator's replica a replica:
//!
//! * `CreateDBInstanceReadReplica` loads the replica's OWN storage from a copy of the
//!   source's, so the replica starts out holding the source's rows;
//! * this module carries the source's COMMITTED state over to that storage again whenever a
//!   session on the source's endpoint commits a change, so the replica follows the source;
//! * the replica's endpoint serves no writes ([`super::read_only`]), so nothing else can
//!   change what it holds;
//! * `PromoteReadReplica` ends the relationship, after which the source names no replica,
//!   nothing is carried over any more, and the two instances' storage diverges — which is
//!   what a promoted replica being a standalone DB instance means.
//!
//! Only COMMITTED state is carried over, and that falls out of how a transaction is kept:
//! a session's uncommitted rows live in that session's own copy of the database
//! ([`super::session_transactions`]) and reach the shared database only at its `COMMIT`, so
//! what a copy of the shared database holds is exactly what the source has committed. A
//! write a transaction has not committed is therefore absent from the replica until the
//! transaction commits.
//!
//! Replication is ASYNCHRONOUS, as the engines' own is: the client's write is answered when
//! the source has it, and the carry-over happens after that answer is computed. A carry-over
//! the engine could not take (either end busy past its copy deadline) leaves the replica
//! behind for the moment and is recorded rather than failing the client's statement — the
//! next committed change carries the whole state over again, which is what closes the gap.

use std::sync::Arc;

use super::sql_engine::InstanceDatabase;
use super::wire_session::SessionTarget;
use super::DbEngineEndpoint;
use crate::state::ComAmazonawsRdsAmazonrdsv19State;

/// Whether a statement whose answer reports the command `verb` can have changed what the
/// replicas of this instance must hold.
///
/// Read off the command NAME the answer reports — the same catalogue of writing commands a
/// read-only endpoint refuses ([`super::read_only::verb_writes`]) — plus the `COMMIT` that
/// publishes a transaction's writes to the shared database. Every other statement (a query,
/// a `SET`, a `SHOW`, a `ROLLBACK`) leaves the source's committed state exactly as the
/// replicas already have it, so nothing is carried over for it.
#[must_use]
pub fn publishes_changes(verb: &str) -> bool {
    super::read_only::verb_writes(verb) || verb == "COMMIT"
}

/// The storage of every read replica of the DB instance `resource_id` names in `region`, in
/// the order the source reports its replicas.
///
/// Read off the SOURCE's own `ReadReplicaDBInstanceIdentifiers` and the engine endpoint each
/// of those identifiers has, so a replica this store no longer holds an endpoint for — one
/// being deleted, or an engine this emulator serves no data plane for — is simply not one of
/// them. Bounded by the read-replica limit the source's engine carries
/// ([`crate::state::read_replica_limit`]).
fn replica_storage(
    store: &ComAmazonawsRdsAmazonrdsv19State,
    region: &str,
    resource_id: &str,
) -> Vec<Arc<InstanceDatabase>> {
    let Some(source) = store.instance_by_resource_id(region, resource_id) else {
        return Vec::new();
    };
    source
        .read_replica_db_instance_identifiers
        .iter()
        .filter_map(|identifier| {
            store.engine_endpoint(&(source.region.clone(), identifier.clone()))
        })
        .map(DbEngineEndpoint::database)
        .collect()
}

/// Carry `source`'s committed databases over to the storage of every read replica of the DB
/// instance `target` names. Does nothing for an endpoint of any other resource, and for an
/// instance that has no replica.
///
/// The store's guard is taken only to read the replicas' database HANDLES and is released
/// before the copy starts; the copy itself is engine work, so it runs on a blocking task
/// under the engine's own copy deadline and page ceiling, exactly as the capture a snapshot
/// takes does. Nothing of the client's statement depends on its outcome.
pub async fn apply_to_read_replicas(
    state: &::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>,
    target: &SessionTarget,
    source: &Arc<InstanceDatabase>,
) {
    let SessionTarget::DbInstance {
        region,
        resource_id,
    } = target
    else {
        return;
    };
    let replicas = replica_storage(&state.lock(), region, resource_id);
    if replicas.is_empty() {
        return;
    }
    let source = Arc::clone(source);
    let carried = ::tokio::task::spawn_blocking(move || {
        replicas
            .iter()
            .map(|replica| replica.load_from(&source))
            .collect::<Vec<_>>()
    })
    .await;
    match carried {
        Err(error) => ::tracing::info!(
            operation = "DbInstanceReadReplicaReplication",
            outcome = "not-carried-over",
            reason = %error,
            "a committed change could not be carried over to this instance's read replicas"
        ),
        Ok(results) => {
            for failure in results.into_iter().filter_map(Result::err) {
                ::tracing::info!(
                    operation = "DbInstanceReadReplicaReplication",
                    outcome = "not-carried-over",
                    reason = %failure.message,
                    "a committed change could not be carried over to a read replica"
                );
            }
        }
    }
}
