// SPDX-License-Identifier: CC0-1.0

//! The DATA plane of one cluster: the PostgreSQL endpoint a client connects to,
//! and what admits a session on it.
//!
//! A cluster publishes a connection endpoint (`CreateCluster`/`GetCluster`'s
//! `endpoint`, "The connection endpoint for the created cluster"), and what that
//! member carries is an address a client afterwards DIALS with a PostgreSQL
//! driver: Aurora DSQL is reached with a stock libpq client, on the database
//! `postgres`, as a database role, with an auth token as the password. So this
//! emulator serves it, on the ONE shared statement of a SQL-wire data plane
//! ([`::service_common::sql_wire_planes`]): the core opens the listener (it owns
//! the socket, the bind interface, the port range, the handshake deadline, the
//! idle timeout and the connection caps), the connections accepted on it are
//! served by the server half of the PostgreSQL frontend/backend protocol, and
//! every statement runs on that cluster's own in-process SQL engine.
//!
//! What this module adds is the Aurora DSQL half of it:
//!
//! * the STORED cluster a session is admitted against ([`StoredClusterFacts`]) —
//!   read out of the store, with every clock-owed transition already applied,
//!   when a login or a statement arrives rather than captured when the listener
//!   opened. So a cluster still being created refuses a session, one whose
//!   deletion has begun refuses one, and a cluster that has gone away ends the
//!   sessions on it, exactly as its own status says;
//! * the CREDENTIAL a login presents, which for this service is an auth token
//!   and not a password this store keeps ([`super::connection_auth`]).
//!
//! The listener is the RESOURCE: the plane is held on the store beside the
//! cluster record, so the port stays open for as long as the cluster does and
//! closes with it — the clock-settled deletion that retires a cluster drops this
//! record, and with it the listener, the sessions on it and the cluster's engine.

use std::sync::Arc;
use std::time::SystemTime;

use ::service_common::sql_wire_planes::{
    self, Concurrency, DatabaseFacts, EngineWire, PresentedLogin, ServedDatabase, ServingState,
    SqlDialect, StatedRefusal,
};

use super::connection_auth;
use super::{ClusterSql, ComAmazonawsDsqlDsqlState, STATUS_ACTIVE, STATUS_IDLE};

/// The SigV4 signing name of this service, which is what names the service
/// answering at an address it opened for an in-process client.
const SIGNING_NAME: &str = "dsql";

/// The PostgreSQL version a cluster's endpoint reports, which is what a driver
/// reads out of the backend's `server_version` parameter.
///
/// Aurora DSQL is a PostgreSQL-COMPATIBLE engine rather than a PostgreSQL
/// server, and what it publishes is the major version it is compatible with
/// ("Aurora DSQL is compatible with PostgreSQL version 16", the Amazon Aurora
/// DSQL User Guide's "PostgreSQL compatibility and feature differences"); the
/// minor is this emulator's own, because no cluster of its own carries one.
const SERVER_VERSION: &str = "16.9";

/// One cluster's open data plane, as the store holds it: where it answers, what
/// answers there, and the engine behind it.
///
/// Dropping it closes the port and severs every session on it.
#[derive(Debug)]
pub struct ClusterDataPlane {
    /// The listener, the wire and the engine, as the shared statement of a
    /// SQL-wire plane holds them.
    plane: sql_wire_planes::OpenedSqlPlane,
    /// The host the cluster's `endpoint` ANNOUNCED when this plane opened, which
    /// is the host an auth token for this cluster is presigned against.
    endpoint_host: String,
}

impl ClusterDataPlane {
    /// Where this plane answers: the address and port the core assigned the
    /// listener, which is where a client of this machine reaches the cluster.
    #[must_use]
    pub fn served_endpoint(&self) -> &sql_wire_planes::ServedEndpoint {
        self.plane.endpoint()
    }

    /// The host the cluster's `endpoint` announces, which an auth token for this
    /// cluster is presigned against.
    #[must_use]
    pub fn endpoint_host(&self) -> &str {
        &self.endpoint_host
    }
}

/// The stored cluster one listener belongs to.
///
/// Holds the store and the cluster's IDENTIFIER rather than the record, so every
/// login and every statement is admitted against the cluster as it stands at
/// that moment — with every clock-owed transition already applied, exactly as a
/// control-plane read sees it.
struct StoredClusterFacts {
    state: ::engine::ServiceState<ComAmazonawsDsqlDsqlState>,
    identifier: String,
    endpoint_host: String,
}

impl ServedDatabase for StoredClusterFacts {
    /// The cluster as the store holds it NOW, with every clock-owed transition
    /// already applied — exactly what a control-plane read sees.
    ///
    /// Settling here can retire a cluster whose deletion has come due, which
    /// removes its plane record and so DROPS the listener, the dialable record
    /// and the engine handle of the very endpoint this session is on. That drop
    /// cannot block under the guard and cannot cut this call short: the core's
    /// listener signals its cancel channel and aborts its ACCEPT task, neither of
    /// which waits on anything (`vendor/server/src/listeners.rs`, `impl Drop for
    /// ResourceListener`), the register of dialable addresses is the store taken
    /// LAST (`service_common::dialable_data_planes`), and the engine is held
    /// behind the session's own handle, so it outlives the record that is gone.
    /// The connection is then severed at this session's next await point, which
    /// is what a client sees when the resource it was using is deleted.
    fn facts(&self) -> Option<DatabaseFacts> {
        let mut held = self.state.lock();
        held.settle(SystemTime::now());
        let cluster = held.clusters.get(&self.identifier)?;
        Some(DatabaseFacts {
            name: cluster.identifier.clone(),
            serving: serving_state(&cluster.status),
            // The administrator role a cluster has from the moment it is
            // created. The password is NOT one this service holds: a session
            // proves itself with an auth token, which is judged by
            // `verified_login` below, and an empty password admits no login
            // through any other path.
            master_username: connection_auth::ADMIN_USER.to_string(),
            master_user_password: String::new(),
            master_database_name: connection_auth::CLUSTER_DATABASE.to_string(),
            server_version: SERVER_VERSION.to_string(),
        })
    }

    /// The DIALECT the engine behind a cluster's endpoint speaks: Aurora DSQL's
    /// ([`ClusterSql`]).
    fn dialect(&self) -> Option<Arc<dyn SqlDialect>> {
        Some(Arc::new(ClusterSql))
    }

    /// How a cluster's engine resolves two sessions writing the same row at the
    /// same time: OPTIMISTICALLY.
    ///
    /// Aurora DSQL holds no row against a writer — each transaction runs on the
    /// snapshot taken when it began, so the second writer's statement answers at
    /// once rather than waiting — and settles the two at COMMIT, where the
    /// transaction whose rows the other changed first is refused
    /// `serialization_failure` and leaves no effect. That is the cluster's own
    /// concurrency control, and this is where the plane states it.
    fn concurrency(&self) -> Concurrency {
        Concurrency::Optimistic
    }

    fn verified_login(&self, login: &PresentedLogin<'_>) -> Option<Result<(), StatedRefusal>> {
        let sql_wire_planes::PasswordProof::Cleartext(presented) = login.proof else {
            // No other wire is served for a cluster: the PostgreSQL startup
            // carries the token itself, which is the only credential this
            // service authenticates.
            return Some(Err(connection_auth::not_this_clusters_credential()));
        };
        if let Err(refused) = connection_auth::admits(
            presented,
            login.user,
            &self.endpoint_host,
            SystemTime::now(),
        ) {
            return Some(Err(refused));
        }
        // The token says its holder may connect AS a role; WHICH roles exist is
        // the database's own answer, and a cluster holds the administrator role
        // and no other until a session creates one — which nothing in this
        // emulator does yet. So a login as any other role is refused the way
        // PostgreSQL refuses a login naming a role it does not hold, rather than
        // opening a session whose `current_user` names a role the cluster has no
        // record of.
        if login.user != connection_auth::ADMIN_USER {
            return Some(Err(connection_auth::role_does_not_exist(login.user)));
        }
        Some(Ok(()))
    }
}

/// Whether a cluster in `status` is serving, as the wire reads it.
///
/// Only a cluster the control plane reports as usable admits a session — the
/// statuses `UpdateCluster` and the clock leave a cluster in when it is up
/// ([`STATUS_ACTIVE`], [`STATUS_IDLE`]). Of the statuses that do not, the wire
/// tells a client which KIND it is: a cluster still being created or still
/// waiting for its multi-Region peering has not finished coming up, and one
/// being updated, deleted or left inactive is on its way out of service.
fn serving_state(status: &str) -> ServingState {
    match status {
        STATUS_ACTIVE | STATUS_IDLE => ServingState::Serving,
        super::STATUS_CREATING | super::STATUS_PENDING_SETUP => ServingState::ComingUp,
        _ => ServingState::GoingDown,
    }
}

/// Open the data plane of the cluster `identifier`, whose connection endpoint
/// announces `endpoint_host`.
///
/// The core binds the socket and assigns the port; this hands it the protocol
/// handler and reports back where it landed. The caller stores the plane on the
/// cluster's store — which is what keeps the port open — so the cluster answers
/// PostgreSQL for as long as it exists.
///
/// `Err` when this emulator's engine has no store or no listener surface, when
/// the core could not bind, or when the SQL engine could not be started: the
/// caller reports no data plane for the cluster rather than a port nothing is
/// behind.
pub async fn open_cluster_data_plane<Eng: Send + Sync + 'static>(
    engine: &Eng,
    identifier: &str,
    endpoint_host: &str,
) -> Result<ClusterDataPlane, String> {
    let state = super::service_state(engine)
        .ok_or_else(|| "this emulator's engine does not vend this service's store".to_string())?;
    let served = Arc::new(StoredClusterFacts {
        state,
        identifier: identifier.to_string(),
        endpoint_host: endpoint_host.to_string(),
    });
    let plane =
        sql_wire_planes::open_sql_wire_plane(engine, EngineWire::Postgres, served, SIGNING_NAME)
            .await?;
    Ok(ClusterDataPlane {
        plane,
        endpoint_host: endpoint_host.to_string(),
    })
}
