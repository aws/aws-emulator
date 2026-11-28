// SPDX-License-Identifier: CC0-1.0

//! The DB instance DATA plane: the engine endpoint `Endpoint{Address,Port}` publishes.
//!
//! A DB instance's endpoint is an ADDRESS A CLIENT CONNECTS TO with its ordinary driver
//! and then speaks the engine's own protocol on — there is no API operation for running
//! SQL against a DB instance. So for every engine whose wire this emulator speaks
//! ([`engine_wire`]) `CreateDBInstance` opens a listener through the CORE
//! (`engine.listeners().tcp_plaintext(..)`, which owns the socket, the bind interface,
//! the port and the TLS material) and the control plane publishes THAT listener's host
//! and port, because an address nothing listens on is not an endpoint. An engine whose
//! wire this emulator does not speak keeps the DNS-form endpoint it has always reported
//! and gets no listener: a port that answered nothing would be worse than a name.
//!
//! The listener is PLAINTEXT on byte 0 and upgrades in band, which is what both wires
//! require: a PostgreSQL client sends `SSLRequest` in the clear and expects the server to
//! answer before any TLS, and a MySQL client reads the server's greeting in the clear and
//! only then sets `CLIENT_SSL`. The handshake itself is run by the core's own
//! [`::emulator_server::TlsUpgrader`], so this crate configures no TLS.
//!
//! What each part owns:
//!
//! * this module — the endpoint record the store holds per instance, which engine speaks
//!   which wire, and the facts a session is admitted against ([`InstanceFacts`]);
//! * [`cluster_endpoint`] — the same for the endpoints of a DB CLUSTER — its writer
//!   endpoint, its reader endpoint and the endpoint of each of its member instances —
//!   whose facts come from the cluster's own record;
//! * [`proxy_endpoint`] — the same for a DB PROXY's endpoint, which runs no engine of its
//!   own: a session on it is served by the database the proxy's target group holds;
//! * [`sql_engine`] — the in-process SQLite database each of an instance's databases runs
//!   its statements against;
//! * [`session_transactions`] — the transaction each SESSION has open on it, and the rows
//!   its `ROLLBACK` puts back;
//! * [`row_locks`] — the rows those transactions hold against each other, and
//!   [`lock_plan`] — which rows one statement asks for;
//! * [`constraint_names`] — the constraint a refused row violated, named as the client's own
//!   engine names it;
//! * [`wire_session`] — what is shared by the two wires: the admission decision, the
//!   server commands (`SHOW`, `SET`) and the session values a statement can read;
//! * [`server_statements`] — the statements the server answers out of the instance's own
//!   catalog: installing an extension, creating a database, opening and ending a
//!   transaction;
//! * [`pg_grants`] — the role memberships and object privileges a PostgreSQL client grants
//!   and revokes, and the catalogs they live in;
//! * [`mysql_accounts`] — the accounts a MySQL client creates, the authentication plugin each
//!   of them authenticates with and the privileges granted to them, and the catalog they live
//!   in;
//! * [`mysql_system_tables`] — the rest of the schema a MySQL server comes up with on disk,
//!   which a snapshot of the instance carries and an export of that snapshot writes out;
//! * [`mysql_s3_load`] — `LOAD DATA FROM S3`, the Amazon S3 import the MySQL engines serve
//!   under the IAM role their parameter group names;
//! * [`pg_aws_s3`] — the `aws_s3` extension's import and export, the Amazon S3 integration
//!   the PostgreSQL engines serve;
//! * [`engine_s3_role`] — WHICH associated role either of those acts under, and whether its
//!   policies allow the object operation;
//! * [`iam_auth_token`] — the authentication token an IAM-authenticated login presents in
//!   place of a password;
//! * [`proxy_client_auth`] — whether a client presenting such a token to a DB PROXY that
//!   requires IAM authentication may connect as the database user it names;
//! * [`read_only`] — the statements an endpoint that serves no writes refuses, and how each
//!   engine words that refusal;
//! * [`pg_code_block`] — the anonymous `DO` block a PostgreSQL client sends, and the server
//!   messages its `RAISE` statements produce;
//! * [`log_export`] — the lines the engine writes to its own log, and the CloudWatch Logs
//!   group a resource with `EnableCloudwatchLogsExports` streams them to;
//! * [`pg_wire`] and [`mysql_wire`] — the server half of each protocol.
//!
//! The endpoint is the RESOURCE: dropping the stored [`DbEngineEndpoint`] closes the port
//! and severs every session on it, which is what a delete does. A stop or a reboot keeps
//! the port (RDS "retains the DB instance's metadata, including its endpoint") and severs
//! the sessions instead ([`DbEngineEndpoint::sever_sessions`]).

use std::sync::atomic::AtomicU32;
use std::sync::Arc;

use tokio::sync::watch;

use super::{ComAmazonawsRdsAmazonrdsv19State, DbInstanceRecord, RegionalKey};
use super::snapshot_data_bytes;

/// What each statement a session ran COST this engine, recorded as the session activity the
/// monitoring services report on.
pub mod activity_samples;
// The CONSISTENT POINTS a DB CLUSTER keeps of what its sessions committed, which
// `BacktrackDBCluster` rewinds it to.
pub mod cluster_backtrack;
pub mod cluster_endpoint;
pub mod constraint_names;
// The CONTINUOUS BACKUP of a DB instance: the recovery points `RestoreDBInstanceToPointInTime`
// serves an instant's data from, and the engine work that records one.
pub mod continuous_backup;
pub mod dialect;
pub mod engine_s3_role;
pub mod engine_time;
pub mod iam_auth_token;
pub mod lock_plan;
pub mod log_export;
pub mod mysql_accounts;
pub mod mysql_columns;
pub mod mysql_diagnostics;
pub mod mysql_insert_id;
pub mod mysql_s3_load;
pub mod mysql_server;
pub mod mysql_strict_mode;
/// The system schema a MySQL server comes up WITH, held on disk like any other table: the
/// grant tables and the help tables, created where that server starts.
pub mod mysql_system_tables;
pub mod mysql_wire;
pub mod pg_aws_s3;
pub mod pg_binary;
pub mod pg_code_block;
pub mod pg_copy;
pub mod pg_grants;
/// The SQL-standard `information_schema` views of a PostgreSQL database: what a driver, an
/// ORM or a migration tool reads the shape of the database out of.
pub mod pg_information_schema;
pub mod pg_numeric;
pub mod pg_server;
pub mod pg_sessions;
pub mod pg_wire;
/// WHAT a client authenticates to a DB PROXY with, where that proxy's `Auth` requires IAM
/// authentication of connections to it, and whether the principal behind its token may
/// connect as the database user it names.
pub mod proxy_client_auth;
/// A DB PROXY's endpoint: the address `DBProxy$Endpoint` publishes, and the sessions a
/// client opens on it.
pub mod proxy_endpoint;
pub mod read_only;
pub mod replication;
pub mod row_locks;
pub mod server_statements;
pub mod session_transactions;
pub mod sql_engine;
// The statement line an engine writes to its own log when its statement logging is on.
pub mod statement_log;
pub mod tls_probe;
pub mod wire_session;

/// WHICH of a resource's endpoints a listener is, which decides whether the sessions on it
/// may write.
///
/// An Amazon RDS DB cluster publishes two addresses for its one engine — the writer
/// endpoint, "the connection endpoint for the primary instance of the DB cluster", and the
/// reader endpoint, which "load-balances connections across the Aurora Replicas that are
/// available in a DB cluster" (read-only instances) — and each of its MEMBER instances
/// publishes "the connection endpoint for the DB instance" of its own. All of them are
/// served by the same wires over the same cluster volume; what differs is whether the
/// session is answered as one on a standby.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointRole {
    /// The endpoint writes go through: a DB instance's own endpoint, a cluster's writer
    /// endpoint.
    Writer,
    /// A READ-ONLY endpoint: a DB cluster's reader endpoint.
    Reader,
    /// The endpoint of ONE MEMBER instance of a DB cluster, named by its
    /// `DBInstanceIdentifier`: whether it serves writes follows which member the cluster
    /// currently writes through, so a `FailoverDBCluster` that moves the writer moves it
    /// under the live sessions of both.
    ClusterMember(String),
    /// A CUSTOM endpoint created in front of a DB cluster, named by its
    /// `DBClusterEndpointIdentifier`: "a custom endpoint for an Aurora cluster represents a
    /// set of DB instances that you choose", so which member a session on it reaches — and
    /// therefore whether it serves writes — follows the endpoint's own type and membership
    /// as they stand NOW, which `ModifyDBClusterEndpoint` changes under the live sessions of
    /// that endpoint ([`ComAmazonawsRdsAmazonrdsv19State::cluster_endpoint_member`]).
    ///
    /// The endpoint is named rather than the member it selects, because the membership is
    /// the endpoint's and moves; the identifier itself is fixed for the endpoint's whole
    /// life, since no operation renames a custom endpoint.
    CustomEndpoint(String),
}

/// Which native wire an engine speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineWire {
    /// The PostgreSQL frontend/backend protocol, version 3.0.
    Postgres,
    /// The MySQL client/server protocol, version 10 (which MariaDB speaks too).
    MySql,
}

/// The wire the engine `engine` speaks, or `None` for an engine this emulator serves no
/// data plane for.
///
/// The engines here are the ones whose DB INSTANCE endpoint is the engine's own endpoint:
/// Amazon RDS for PostgreSQL, for MySQL and for MariaDB — MariaDB speaking the MySQL
/// protocol. The Oracle, SQL Server and Db2 engines speak neither protocol, and this
/// emulator runs no engine for them.
///
/// An AURORA DB instance is deliberately not here: it runs no engine of its own, so its
/// endpoint is a second address onto its DB CLUSTER's volume, opened with the cluster's wire
/// and the cluster's storage ([`ComAmazonawsRdsAmazonrdsv19State::cluster_volume_of`]) and
/// served in the [`EndpointRole::ClusterMember`] role.
#[must_use]
pub fn engine_wire(engine: &str) -> Option<EngineWire> {
    match engine.to_ascii_lowercase().as_str() {
        "postgres" => Some(EngineWire::Postgres),
        "mysql" | "mariadb" => Some(EngineWire::MySql),
        _ => None,
    }
}

/// The first version of RDS for PostgreSQL whose default parameter group turns
/// `rds.force_ssl` on.
///
/// Amazon RDS publishes the parameter's default as 1 (on) for RDS for PostgreSQL 15 and
/// higher and 0 (off) for the earlier versions, so a plaintext login is refused by
/// default on an instance of 15 or later and admitted on one before it.
const FORCE_SSL_DEFAULT_FROM_MAJOR_VERSION: u32 = 15;

/// Whether a PostgreSQL instance running `engine_version` forces TLS by default; see
/// [`FORCE_SSL_DEFAULT_FROM_MAJOR_VERSION`]. A version this cannot read is treated as a
/// current one, which is the default a new instance gets.
///
/// The one place that rule lives: the login gate above reads it for an INSTANCE's engine
/// version, and the published record the parameter catalog reports for a DB parameter
/// group FAMILY — the parameter's default AND its apply type, both of which moved at this
/// release — is the same rule over that family's version
/// (`crate::state::published_spec_for_family`), so for RDS for PostgreSQL, whose default
/// this release moved, what a describe reports for `rds.force_ssl` and what a session on
/// such an instance is held to cannot disagree.
pub(crate) fn postgres_forces_ssl_by_default(engine_version: &str) -> bool {
    engine_version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .is_none_or(|major| major >= FORCE_SSL_DEFAULT_FROM_MAJOR_VERSION)
}

/// WHICH parameter decides that a session on `wire` must be encrypted: `rds.force_ssl` on
/// PostgreSQL and `require_secure_transport` on MySQL and MariaDB.
#[must_use]
pub(crate) fn secure_transport_parameter(wire: EngineWire) -> &'static str {
    match wire {
        EngineWire::Postgres => "rds.force_ssl",
        EngineWire::MySql => "require_secure_transport",
    }
}

/// Whether an encrypted session is REQUIRED, from the value the resource's parameter group
/// sets for [`secure_transport_parameter`] (`None` where it sets none) and the engine
/// version the resource runs.
///
/// The ONE reading of that parameter, for a DB instance's endpoint and a DB cluster's
/// alike, so the two cannot disagree about a login this emulator admits:
///
/// * PostgreSQL: Amazon RDS documents `rds.force_ssl`'s default as 1 for version 15 and
///   higher and 0 for the versions before it, so the resource's own major version decides
///   it where its group sets nothing.
/// * MySQL and MariaDB: `require_secure_transport`, whose documented default is OFF.
#[must_use]
pub(crate) fn secure_transport_in_force(
    wire: EngineWire,
    set: Option<&str>,
    engine_version: Option<&str>,
) -> bool {
    match set {
        Some(value) => matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "on" | "true" | "yes"
        ),
        None => match wire {
            EngineWire::Postgres => {
                postgres_forces_ssl_by_default(engine_version.unwrap_or_default())
            }
            EngineWire::MySql => false,
        },
    }
}

/// The address a DB instance's engine listener accepts connections on, which is what the
/// modelled `Endpoint` reports for an instance that has one.
#[derive(Debug, Clone)]
pub struct ServedEndpoint {
    /// The host the core bound the listener to.
    pub host: String,
    /// The port the core assigned it. The port a request ASKED for (`Port`,
    /// `DBPortNumber`) is kept on the record as the instance's configured port, but the
    /// core owns the port range, so the published port is the one a client can reach.
    pub port: i32,
}

/// One DB instance's engine endpoint: the live listener, the database behind it, and the
/// handle that severs its sessions.
pub struct DbEngineEndpoint {
    /// Where the control plane publishes it.
    pub endpoint: ServedEndpoint,
    /// Where this engine's READ-ONLY endpoint listens, for a resource that has one: a DB
    /// cluster's reader endpoint. `None` for a resource that opens only the one listener
    /// ([`EndpointRole`]).
    ///
    /// The control plane publishes the HOST of this listener as the cluster's
    /// `ReaderEndpoint` and ONE `Port` for both endpoints — its engine endpoint's — which is
    /// all the modelled envelope carries for the pair
    /// ([`crate::state::DbClusterRecord::reader_endpoint`]).
    pub reader: Option<ServedEndpoint>,
    /// Which protocol the listener speaks.
    pub wire: EngineWire,
    /// The instance's databases, shared with every session on the listener.
    database: Arc<sql_engine::InstanceDatabase>,
    /// The sessions open on this listener, shared with every session on it: what
    /// `pg_stat_activity` reports and what `max_connections` is counted against.
    sessions: pg_sessions::SessionRegistry,
    /// The engine's run: a session serves only while the value it started under is
    /// current, so bumping it severs every live session (a reboot, a stop).
    generation: watch::Sender<u64>,
    /// Held only to keep the port open; the core owns the socket and its TLS.
    _listener: ::emulator_server::ResourceListener,
    /// The same, for the read-only endpoint of this engine where it has one: dropping the
    /// record closes both ports.
    _reader_listener: Option<::emulator_server::ResourceListener>,
    /// The records that say these addresses are ones this emulator has opened a plane at, so
    /// an in-process client of this emulator that was handed one — the code of a Lambda
    /// function attached to the instance's VPC — can open a connection to it, served by the
    /// same handler as the listener itself (`service_common::dialable_data_planes`). Dropping
    /// them withdraws the addresses, exactly as dropping the listeners above closes the ports.
    _dialable: Vec<::service_common::dialable_data_planes::OpenedPlanes>,
}

/// Written by hand rather than derived: the endpoint holds a core `ResourceListener` and a
/// database connection, neither of which is `Debug`. What a reader needs is the address it
/// published and the protocol it speaks.
impl std::fmt::Debug for DbEngineEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbEngineEndpoint")
            .field("endpoint", &self.endpoint)
            .field("reader", &self.reader)
            .field("wire", &self.wire)
            .field("generation", &*self.generation.borrow())
            .finish()
    }
}

impl DbEngineEndpoint {
    /// The database every session on this endpoint runs its statements against.
    #[must_use]
    pub fn database(&self) -> Arc<sql_engine::InstanceDatabase> {
        Arc::clone(&self.database)
    }

    /// The sessions open on this endpoint.
    #[must_use]
    pub fn sessions(&self) -> pg_sessions::SessionRegistry {
        self.sessions.clone()
    }

    /// End every session open on this endpoint, leaving the port bound.
    ///
    /// What a restart of the database engine does: "Rebooting a DB instance results in a
    /// momentary outage" and a stopped instance serves nobody, so the connections an
    /// engine held do not survive either. The work none of them committed does not survive
    /// it either: the engine's run ends, so every transaction still open is rolled back here
    /// rather than waiting for each severed session's own teardown to get to it
    /// ([`sql_engine::InstanceDatabase::rollback_open_transactions`], which is what each
    /// session's teardown then finds nothing left to do).
    pub fn sever_sessions(&self) {
        let next = self.generation.borrow().wrapping_add(1);
        let _ = self.generation.send(next);
        self.database.rollback_open_transactions();
    }
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// The engine endpoint of the DB instance `key` names, for the instances that have
    /// one.
    #[must_use]
    pub fn engine_endpoint(&self, key: &RegionalKey) -> Option<&DbEngineEndpoint> {
        self.db_engine_endpoints.get(key)
    }

    /// End every session open on the DB instance `key` names. Does nothing for an
    /// instance with no engine endpoint.
    pub fn sever_engine_sessions(&self, key: &RegionalKey) {
        if let Some(endpoint) = self.db_engine_endpoints.get(key) {
            endpoint.sever_sessions();
        }
    }

    /// Move an instance's engine endpoint with the instance when `ModifyDBInstance`
    /// renames it, so the listener a client is connected to stays the endpoint the
    /// control plane publishes for that instance.
    pub fn move_engine_endpoint(&mut self, from: &RegionalKey, to: &RegionalKey) {
        if from == to {
            return;
        }
        if let Some(endpoint) = self.db_engine_endpoints.remove(from) {
            self.db_engine_endpoints.insert(to.clone(), endpoint);
        }
    }

    /// Whether a session on `record`'s engine endpoint must be encrypted.
    ///
    /// The engines carry this as a PARAMETER, so the answer is the effective value of
    /// that parameter for this instance: what the instance's own DB parameter group sets
    /// for it, and the engine's default where the group sets nothing (a group this store
    /// holds no record of is a default group AWS provides, which sets no parameter of its
    /// own).
    ///
    /// * PostgreSQL: `rds.force_ssl`. Amazon RDS documents the default as 1 for RDS for
    ///   PostgreSQL version 15 and higher and 0 for the versions before it, so the
    ///   instance's own major version decides it.
    /// * MySQL and MariaDB: `require_secure_transport`, whose documented default is OFF.
    #[must_use]
    pub fn requires_secure_transport(&self, record: &DbInstanceRecord) -> bool {
        let Some(wire) = engine_wire(&record.engine) else {
            return false;
        };
        let set = record
            .db_parameter_group_name
            .as_ref()
            .and_then(|name| {
                self.db_parameter_groups
                    .get(&(record.region.clone(), name.to_ascii_lowercase()))
            })
            .and_then(|group| group.parameters.get(secure_transport_parameter(wire)));
        secure_transport_in_force(
            wire,
            set.map(|setting| setting.value.as_str()),
            record.engine_version.as_deref(),
        )
    }

    /// The DB instance in `region` whose `DbiResourceId` is `resource_id`.
    ///
    /// A session on an engine endpoint resolves its instance by this id rather than by
    /// identifier, because `ModifyDBInstance` can rename an instance under a live
    /// connection while the resource id "never changes". Reads one region's ordered range,
    /// which the region's own instance quota bounds.
    #[must_use]
    pub fn instance_by_resource_id(
        &self,
        region: &str,
        resource_id: &str,
    ) -> Option<&DbInstanceRecord> {
        self.db_instances
            .range((region.to_string(), String::new())..)
            .take_while(|((stored, _), _)| stored == region)
            .map(|(_, record)| record)
            .find(|record| record.dbi_resource_id == resource_id)
    }

    /// The same instance, held MUTABLY: what a session writes through when the engine
    /// behind the endpoint records something on the instance it belongs to — a line of its
    /// log ([`statement_log`]), a recovery point of its continuous backup
    /// ([`continuous_backup`]).
    pub fn instance_by_resource_id_mut(
        &mut self,
        region: &str,
        resource_id: &str,
    ) -> Option<&mut DbInstanceRecord> {
        self.db_instances
            .range_mut((region.to_string(), String::new())..)
            .take_while(|((stored, _), _)| stored == region)
            .map(|(_, record)| record)
            .find(|record| record.dbi_resource_id == resource_id)
    }
}

/// A COPY of the data the DB instance `key` names holds right now: what a DB snapshot of
/// that instance holds, so restoring the snapshot brings those rows back.
///
/// `Ok(None)` for an instance with no engine endpoint — an engine whose wire this emulator
/// serves no data plane for ([`engine_wire`]), which has no storage to copy, and an
/// identifier this store holds no instance under. `Err` when the engine could not make the
/// copy, which the caller answers as a failure of this emulator rather than storing a
/// snapshot that holds nothing.
///
/// The store lock is taken only to read the instance's database HANDLE and is released
/// before the copy starts, and the copy itself runs on a blocking task — it is engine
/// work, like a statement a session runs, so it belongs off the runtime's worker threads
/// and outside the lock every other request to this service needs. Its own bounds are
/// [`sql_engine::COPY_DEADLINE`] and the page ceiling each database carries.
///
/// Called BEFORE the handler takes the store lock for the work the snapshot is part of, so
/// no state guard is held across its await.
pub async fn capture_instance_storage(
    state: &::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>,
    key: &RegionalKey,
) -> Result<Option<Arc<sql_engine::InstanceDatabase>>, String> {
    let database = state
        .lock()
        .engine_endpoint(key)
        .map(DbEngineEndpoint::database);
    capture_storage(database, "DB instance").await
}

/// The full size of the data a captured copy holds, summed OFF the runtime's worker
/// threads and before the caller takes the store lock ([`snapshot_data_bytes`]).
///
/// Summed exactly once, where the copy is taken: the copy never changes afterwards, so the
/// snapshot's record keeps the answer and no later read of that snapshot re-reads its data.
/// It is engine work under the same contract as the capture above — a blocking task, no
/// state guard held across it — and `Err` carries what went wrong, which the caller answers
/// as a failure of this emulator rather than storing a size nothing measured.
pub async fn captured_data_bytes(
    storage: Option<&Arc<sql_engine::InstanceDatabase>>,
) -> Result<i64, String> {
    let Some(storage) = storage.cloned() else {
        return Ok(0);
    };
    ::tokio::task::spawn_blocking(move || snapshot_data_bytes(Some(&storage)))
        .await
        .map_err(|error| format!("the size of the snapshot's data could not be read: {error}"))
}

/// A COPY of the data the DB CLUSTER `key` names holds right now: what a DB cluster
/// snapshot of that cluster holds, so restoring the snapshot brings those rows back.
///
/// The cluster sibling of [`capture_instance_storage`], under the same contract: `Ok(None)`
/// for a cluster with no engine endpoint — an engine whose wire this emulator serves no
/// data plane for ([`cluster_endpoint::cluster_engine_wire`]), and an identifier this store
/// holds no cluster under — and `Err` when the engine could not make the copy, which the
/// caller answers as a failure of this emulator rather than storing a snapshot that holds
/// nothing. The store lock is taken only to read the database HANDLE, the copy runs on a
/// blocking task, and the call belongs BEFORE the handler takes the store lock for the work
/// the snapshot is part of.
///
/// This is the cluster's WIRE storage. The databases it holds through its Data API HTTP
/// endpoint are that service's own, and are captured beside this one
/// ([`super::capture_data_api_cluster_data`]).
pub async fn capture_cluster_storage(
    state: &::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>,
    key: &RegionalKey,
) -> Result<Option<Arc<sql_engine::InstanceDatabase>>, String> {
    let database = state
        .lock()
        .cluster_engine_endpoint(key)
        .map(DbEngineEndpoint::database);
    capture_storage(database, "DB cluster").await
}

/// The copy of a DB cluster's DATA an export reads: a copy of the cluster's wire storage
/// (`storage`) with the tables of `data_api` — a captured copy of the databases the cluster
/// holds through its Data API HTTP endpoint ([`super::capture_data_api_cluster_data`], the
/// same copy a snapshot of that cluster carries) — carried into it.
///
/// A cluster's data is what its clients WROTE, through whichever of its endpoints they wrote
/// it, so an export of that cluster or of a snapshot of it writes out both. It is a COPY
/// throughout: neither the live storage nor the snapshot's own is written to, so an export
/// changes nothing a later caller sees. A cluster this emulator keeps no wire storage for
/// still has the Data API's tables, so the copy those are carried into is opened here rather
/// than left absent; `Ok(None)` means the cluster has neither.
///
/// `default_database` is the cluster's own `DatabaseName`; see
/// [`super::data_api_rows::carry_data_api_tables`] for what it decides. Both the copy and the
/// carry are engine work on a blocking task, exactly as the capture above is.
pub async fn capture_cluster_data(
    storage: Option<Arc<sql_engine::InstanceDatabase>>,
    data_api: Option<Arc<super::DataApiClusterData>>,
    default_database: Option<&str>,
) -> Result<Option<Arc<sql_engine::InstanceDatabase>>, String> {
    let copy = capture_storage(storage, "DB cluster").await?;
    let Some(data_api) = data_api else {
        return Ok(copy);
    };
    let copy = match copy {
        Some(copy) => copy,
        None => Arc::new(sql_engine::InstanceDatabase::open().map_err(|error| {
            format!("the DB cluster's data could not be read: {}", error.message)
        })?),
    };
    let into = Arc::clone(&copy);
    let default_database = default_database.map(ToString::to_string);
    let carried = ::tokio::task::spawn_blocking(move || {
        super::data_api_rows::carry_data_api_tables(&data_api, default_database.as_deref(), &into)
    })
    .await
    .map_err(|error| format!("the DB cluster's data could not be copied: {error}"))?;
    // How many tables were carried is the carry's own account of what it did; what the caller
    // acts on is whether a database it could not read refused it.
    let _tables = carried?;
    Ok(Some(copy))
}

/// A COPY of the data an endpoint that has just been opened holds, for a caller that has
/// the endpoint in hand rather than in the store: what a DB cluster's EARLIEST backtrack
/// point holds, taken where the cluster's listener is opened.
///
/// Same contract as the two captures above: the copy runs on a blocking task and `Err`
/// carries what went wrong, which the caller answers as a failure of this emulator rather
/// than recording a backtrack point that holds nothing.
pub async fn capture_endpoint_storage(
    endpoint: &DbEngineEndpoint,
) -> Result<Option<Arc<sql_engine::InstanceDatabase>>, String> {
    capture_storage(Some(endpoint.database()), "DB cluster").await
}

/// Put the data the DB CLUSTER `key` names back to the copy `source` holds, which is what
/// BACKTRACKING a cluster does: "rewinds the DB cluster to the time you specify", so the
/// rows a session reads afterwards are the rows the cluster held at that time.
///
/// The live storage is REPLACED rather than merged into
/// ([`sql_engine::InstanceDatabase::load_from`]), the handle is read under the store's guard
/// and the guard is released before the copy is awaited, and the copy itself runs on a
/// blocking task under the engine's own deadline — the same contract the captures above
/// hold. `Ok(())` for a cluster with no engine endpoint, which has no data to rewind.
pub async fn rewind_cluster_storage(
    state: &::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>,
    key: &RegionalKey,
    source: Arc<sql_engine::InstanceDatabase>,
) -> Result<(), String> {
    let Some(live) = state
        .lock()
        .cluster_engine_endpoint(key)
        .map(DbEngineEndpoint::database)
    else {
        return Ok(());
    };
    ::tokio::task::spawn_blocking(move || live.load_from(&source))
        .await
        .map_err(|error| format!("the DB cluster's storage could not be rewound: {error}"))?
        .map_err(|error| {
            format!(
                "the DB cluster's storage could not be rewound: {}",
                error.message
            )
        })
}

/// The copy itself, for a resource whose database handle has already been read out of the
/// store: the one body the two captures above share, so a DB instance's snapshot and a DB
/// cluster's are taken by the same engine call under the same bounds and report the same
/// failure, each naming its own `resource`.
///
/// `database` is `None` for a resource with no engine endpoint, which has no storage to copy
/// — the `Ok(None)` both callers document. The handle is read under the store's guard by the
/// caller and the guard is released before this is awaited, so no state lock is held across
/// the copy.
async fn capture_storage(
    database: Option<Arc<sql_engine::InstanceDatabase>>,
    resource: &str,
) -> Result<Option<Arc<sql_engine::InstanceDatabase>>, String> {
    let Some(database) = database else {
        return Ok(None);
    };
    let copied = ::tokio::task::spawn_blocking(move || database.capture())
        .await
        .map_err(|error| format!("the {resource}'s storage could not be copied: {error}"))?;
    copied.map(|copy| Some(Arc::new(copy))).map_err(|error| {
        format!(
            "the {resource}'s storage could not be copied: {}",
            error.message
        )
    })
}

/// Open the engine endpoint of `target` — a DB instance's own endpoint or a DB cluster's
/// writer endpoint — on the wire its engine speaks, plus the resource's READ-ONLY endpoint
/// where it publishes one (`reader`).
///
/// The core binds the socket and assigns the port; this only hands it the protocol
/// handler and reports back where it landed. The caller stores the returned endpoint in
/// the state — which is what keeps the port open — and publishes
/// [`DbEngineEndpoint::endpoint`] as the resource's `Endpoint` and
/// [`DbEngineEndpoint::reader`] as its reader endpoint.
///
/// The read-only endpoint is a SECOND listener over the SAME storage, which is what an
/// Aurora reader endpoint is: "the reader endpoint ... load-balances connections across the
/// Aurora Replicas", and every instance of an Aurora cluster reads one cluster volume. So
/// the rows the writer endpoint committed are the rows a session on the reader endpoint
/// reads, and the difference between the two is that the reader's sessions may not write
/// ([`EndpointRole`]).
///
/// `Err` when this service was mounted on an engine with no listener surface, or when the
/// core could not bind: the caller answers that with a modelled failure rather than
/// publishing an address nothing serves.
/// WHAT to open: which resource's endpoint it is, on which wire, in which role, and over
/// which storage.
///
/// One struct rather than five arguments, because the five belong together: the role and the
/// storage are what make a cluster MEMBER's endpoint a second address onto the cluster's
/// volume rather than an engine of its own.
pub struct EngineEndpointPlan {
    /// The resource whose record a session on this endpoint is admitted against.
    pub target: wire_session::SessionTarget,
    /// The wire its engine speaks.
    pub wire: EngineWire,
    /// WHICH of that resource's endpoints the listener is.
    pub role: EndpointRole,
    /// Open the resource's READ-ONLY endpoint beside the writing one, which a DB cluster
    /// publishes as its reader endpoint.
    pub reader: bool,
    /// The storage to serve: the volume of the DB cluster an endpoint belongs to, where
    /// that storage already exists. `None` starts the resource's own engine.
    pub storage: Option<Arc<sql_engine::InstanceDatabase>>,
    /// The in-process callback of the operation opening this endpoint
    /// (`OperationContext::callback`), which every session on it is served with: a session
    /// has no request context of its own, and this is what the engine's log lines are
    /// delivered to CloudWatch Logs over ([`log_export`]).
    pub callback: ::smithy_server_runtime::EmbeddedEmulator,
}

/// Open the engine endpoint `plan` describes, and the resource's READ-ONLY endpoint where it
/// publishes one ([`EngineEndpointPlan::reader`]).
///
/// The core binds the socket and assigns the port; this only hands it the protocol
/// handler and reports back where it landed. The caller stores the returned endpoint in
/// the state — which is what keeps the port open — and publishes
/// [`DbEngineEndpoint::endpoint`] as the resource's `Endpoint` and
/// [`DbEngineEndpoint::reader`] as its reader endpoint.
///
/// The read-only endpoint is a SECOND listener over the SAME storage, which is what an
/// Aurora reader endpoint is: "the reader endpoint ... load-balances connections across the
/// Aurora Replicas", and every instance of an Aurora cluster reads one cluster volume. So
/// the rows the writer endpoint committed are the rows a session on the reader endpoint
/// reads, and the difference between the two is that the reader's sessions may not write
/// ([`EndpointRole`]).
///
/// `Err` when this service was mounted on an engine with no listener surface, or when the
/// core could not bind: the caller answers that with a modelled failure rather than
/// publishing an address nothing serves.
pub async fn open_engine_endpoint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>,
    plan: EngineEndpointPlan,
) -> Result<DbEngineEndpoint, String> {
    let EngineEndpointPlan {
        target,
        wire,
        role,
        reader,
        storage,
        callback,
    } = plan;
    let listeners = shared_engine(engine)
        .ok_or_else(|| "this emulator's engine has no listener surface".to_string())?
        .listeners();
    // Whether this call STARTS the engine behind the endpoint, rather than opening a second
    // address onto storage that is already running: what a server comes up with is created
    // once, where it comes up.
    let starting = storage.is_none();
    let database = match storage {
        Some(shared) => shared,
        None => {
            // WHERE this endpoint's databases live. A DB CLUSTER's are held in the
            // cluster's own VOLUME — the storage every surface of that cluster addresses,
            // its Data API HTTP endpoint among them
            // (`service_common::cluster_sql_volumes`) — so a table a client created
            // through one door is a table a session here selects, and the other way round.
            // A DB instance's engine and a DB proxy's endpoint serve storage of their own:
            // a proxy is handed the database its target group holds, and an instance runs
            // its own engine.
            let volume = match &target {
                wire_session::SessionTarget::DbCluster { resource_id, .. } => {
                    ::service_common::cluster_sql_volumes::ClusterVolumes::of(engine)
                        .map(|volumes| volumes.cluster(resource_id))
                }
                wire_session::SessionTarget::DbInstance { .. }
                | wire_session::SessionTarget::DbProxy { .. } => None,
            };
            Arc::new(
                sql_engine::InstanceDatabase::open_on_volume(volume).map_err(|error| {
                    format!("the SQL engine could not be started: {}", error.message)
                })?,
            )
        }
    };
    // What a server comes up WITH on disk is created where the server starts, rather than
    // when a statement first names it: a MySQL-family server's own schema and its tables, and
    // the database a PostgreSQL server's own initialisation creates.
    if starting {
        match wire {
            EngineWire::MySql => mysql_system_tables::start_system_schema(&database),
            EngineWire::Postgres => pg_server::start_server_database(&database),
        }
    }
    let (generation, generation_rx) = watch::channel(0u64);
    let sessions = pg_sessions::SessionRegistry::default();
    let next_connection_id = Arc::new(AtomicU32::new(1));
    // What a session on either of this engine's endpoints is served from, built per
    // endpoint so each carries the role of the address the client dialled. The TLS server
    // is the core's, taken per listener.
    let context_for = |role: EndpointRole| -> Result<wire_session::SessionContext, String> {
        Ok(wire_session::SessionContext {
            state: state.clone(),
            target: target.clone(),
            role,
            database: Some(Arc::clone(&database)),
            sessions: sessions.clone(),
            upgrader: listeners.tls_upgrader().map_err(|error| {
                format!("the engine endpoint's TLS could not be prepared: {error}")
            })?,
            // The object store the S3 import statements read through, on the same engine
            // this service is mounted on: what Amazon S3's own `PutObject` wrote is what
            // such a statement loads.
            objects: mysql_s3_load::object_store(engine),
            // And the IAM store the role those statements act under is resolved against, on
            // that same engine.
            iam: engine_s3_role::iam_store(engine),
            // A session on a resource's OWN endpoint reaches no proxy, so no proxy decides
            // its client's credential; the authorizer is carried all the same, because what
            // it reads are two other services' stores on this same engine and an endpoint
            // opened here outlives every request that changes a policy.
            connect_authorizer: proxy_client_auth::ConnectAuthorizer::of_engine(engine),
            generation: generation_rx.clone(),
            next_connection_id: Arc::clone(&next_connection_id),
            callback: callback.clone(),
            // The SHARED activity store on that same engine, where every statement a
            // session of this endpoint runs leaves what it cost — the record Performance
            // Insights reports this resource's load from
            // ([`activity_samples`]).
            activity: activity_samples::ActivityStore::of(engine),
        })
    };
    let (listener, writing) = open_wire_listener(&listeners, wire, context_for(role)?).await?;
    let reader_endpoint = match reader {
        true => Some(
            open_wire_listener(&listeners, wire, context_for(EndpointRole::Reader)?).await?,
        ),
        false => None,
    };
    let (reader_listener, reading) = match reader_endpoint {
        Some((listener, plane)) => (Some(listener), Some(plane)),
        None => (None, None),
    };
    // The addresses are now ones this emulator SERVES: each is recorded as the address its
    // own plane answers at, and the records are withdrawn with the listeners themselves.
    let dialable: Vec<::service_common::dialable_data_planes::OpenedPlanes> = [
        Some((&listener, &writing)),
        reader_listener.as_ref().zip(reading.as_ref()),
    ]
    .into_iter()
    .flatten()
    .filter_map(|(listener, plane)| dialable_plane(engine, listener, plane))
    .collect();
    Ok(DbEngineEndpoint {
        endpoint: ServedEndpoint {
            host: listener.host(),
            port: i32::from(listener.port()),
        },
        reader: reader_listener.as_ref().map(|listener| ServedEndpoint {
            host: listener.host(),
            port: i32::from(listener.port()),
        }),
        wire,
        database,
        sessions,
        generation,
        _listener: listener,
        _reader_listener: reader_listener,
        _dialable: dialable,
    })
}

/// Open ONE plaintext listener speaking `wire`, served by that wire's own protocol handler
/// over `context`, and answer with the listener and the SAME handler as a plane an
/// in-process client can open a connection to.
///
/// ONE protocol handler per endpoint, served at both of its ends: the listener the core
/// accepts a client's connections on, and the connections an IN-PROCESS client of this
/// emulator opens to the address this endpoint publishes
/// (`service_common::dialable_data_planes`) — a handler running in a Lambda function
/// attached to this resource's VPC reads the endpoint out of the control plane and opens a
/// connection to exactly that address. Both ends are the same handler over the same session
/// context, so a session is served the same way whichever end it arrived on.
///
/// The one place either kind of endpoint this module opens — a resource's own engine
/// endpoint ([`open_engine_endpoint`]) and a DB proxy's ([`proxy_endpoint`]) — binds a
/// listener, so the two cannot differ in what byte 0 of a connection meets.
pub(crate) async fn open_wire_listener(
    listeners: &::emulator_server::Listeners,
    wire: EngineWire,
    context: wire_session::SessionContext,
) -> Result<
    (
        ::emulator_server::ResourceListener,
        Arc<dyn ::service_common::dialable_data_planes::DialablePlane>,
    ),
    String,
> {
    let opened = match wire {
        EngineWire::Postgres => {
            let served = Arc::new(pg_wire::PostgresWire::new(context));
            listeners
                .tcp_plaintext(::service_common::dialable_data_planes::SharedHandler(
                    Arc::clone(&served),
                ))
                .await
                .map(|listener| {
                    // The wire negotiates its own encryption in band, so the plane is
                    // handed byte 0 at both ends and upgrades it itself.
                    let plane =
                        ::service_common::dialable_data_planes::served_in_process(served, None);
                    (listener, plane)
                })
        }
        EngineWire::MySql => {
            let served = Arc::new(mysql_wire::MySqlWire::new(context));
            listeners
                .tcp_plaintext(::service_common::dialable_data_planes::SharedHandler(
                    Arc::clone(&served),
                ))
                .await
                .map(|listener| {
                    let plane =
                        ::service_common::dialable_data_planes::served_in_process(served, None);
                    (listener, plane)
                })
        }
    };
    opened.map_err(|error| format!("the engine endpoint could not be opened: {error}"))
}

/// Record the address `listener` landed on as one `plane` answers at, so an in-process
/// client of this emulator that was handed that address reaches this endpoint.
///
/// `None` for an engine that vends no such records, which opens no address; the returned
/// record is held beside the listener, so dropping it withdraws the address exactly as
/// dropping the listener closes the port.
pub(crate) fn dialable_plane<Eng: Send + Sync + 'static>(
    engine: &Eng,
    listener: &::emulator_server::ResourceListener,
    plane: &Arc<dyn ::service_common::dialable_data_planes::DialablePlane>,
) -> Option<::service_common::dialable_data_planes::OpenedPlanes> {
    dialable_plane_also_at(engine, listener, Vec::new(), plane)
}

/// [`dialable_plane`] for an endpoint that is also answered at addresses OTHER than the one
/// the core bound its listener on: `also` are those addresses, recorded beside it.
///
/// What needs them is an endpoint whose published member carries NO PORT — a DB proxy's
/// ([`proxy_endpoint`]) — so the client completes the address with the engine family's own
/// port rather than with the one the core assigned. Both addresses answer the SAME handler
/// over the same session context, and the whole set is withdrawn together when the returned
/// record is dropped.
pub(crate) fn dialable_plane_also_at<Eng: Send + Sync + 'static>(
    engine: &Eng,
    listener: &::emulator_server::ResourceListener,
    also: impl IntoIterator<Item = String>,
    plane: &Arc<dyn ::service_common::dialable_data_planes::DialablePlane>,
) -> Option<::service_common::dialable_data_planes::OpenedPlanes> {
    let planes = ::service_common::dialable_data_planes::DialablePlanes::of(engine)?;
    let mut authorities = vec![::service_common::data_plane_addressing::listener_authority(
        listener,
    )];
    authorities.extend(also);
    Some(planes.opening(authorities, SIGNING_NAME, plane))
}

/// The SigV4 signing name of this service, which is what the records above name as the
/// service answering at an address it opened.
const SIGNING_NAME: &str = "rds";

/// The SHARED engine this service is mounted on, which owns the listener surface: call it
/// with a handler's `&*self.engine`.
fn shared_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<&::engine::InMemoryEngine> {
    (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()
}
