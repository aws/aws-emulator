// SPDX-License-Identifier: CC0-1.0

//! The DATA PLANE of one elastic cluster: the address a client connects to, the documents
//! behind it, and the server that answers there.
//!
//! `Cluster$clusterEndpoint` is "The URL used to connect to the elastic cluster"
//! (`smithy_query(query="shape", shape="Cluster")`), and it is `@required` on every
//! cluster-returning operation. It is not a label a caller reads back but an address a client
//! is CONFIGURED with: a driver takes it, opens a connection on the document port and speaks
//! the MongoDB wire protocol there. A name under Amazon DocumentDB's own DNS zone resolves to
//! nothing on the machine this emulator runs on, so a cluster reporting one published an
//! address no client could reach — and sent its own client off this emulator.
//!
//! So every elastic cluster this emulator creates is given a socket of the EMULATOR's own,
//! opened through the core (`engine.listeners().tcp`, which owns the socket, the bind
//! interface, the port range, the TLS material and the connection caps), and what
//! `clusterEndpoint` carries is the `host:port` that socket answers at — both halves, because
//! the member is one value and the port the core assigned is the port a client has to dial.
//! The socket is TLS from the first byte, which is how a client of an elastic cluster
//! connects: an elastic cluster's in-transit encryption cannot be turned off.
//!
//! That same address is recorded as one this emulator serves in process
//! (`service_common::dialable_data_planes`), so a client of this emulator that can open no
//! host socket — the code of a function attached to the cluster's virtual private cloud, which
//! is the vantage point such an endpoint is reachable from on AWS — reaches the same server at
//! the same address.
//!
//! # What answers there
//!
//! An elastic cluster speaks the SAME wire as an Amazon DocumentDB cluster, so the protocol,
//! the document engine, the SASL machinery and the refusal shapes are the ones that service
//! already serves (`service_com_amazonaws_docdb_amazonrdsv19::state::cluster_data_plane`):
//! this module is the ELASTIC endpoint of that wire, and holds only what an elastic cluster
//! does differently —
//!
//! * [`commands`] — which commands a session may send before it has authenticated, and the
//!   refusals an elastic cluster words itself;
//! * [`sharding`] — `shardCollection`, the one shard key an elastic cluster admits, and the
//!   rules every write of a sharded collection holds to;
//! * [`sharded_writes`] — how a write one of those rules refuses is reported;
//! * [`session`] — what ONE connection holds while it is open.
//!
//! Everything else — the message framing, the documents, the query engine, the SCRAM exchange
//! — is the engine's own and is served through that seam
//! ([`::service_com_amazonaws_docdb_amazonrdsv19::state::cluster_data_plane::wire::WireEndpoint`]).
//!
//! The socket's life is the cluster's: the handle lives on this store under the cluster's own
//! ARN, and dropping it (the delete) closes the port and severs every connection on it. The
//! cluster's DOCUMENTS go with it, because they are that cluster's storage.

pub mod commands;
pub mod refusal;
pub mod session;
pub mod sharded_writes;
pub mod sharding;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use ::service_com_amazonaws_docdb_amazonrdsv19::state::cluster_data_plane as document_engine;

pub use document_engine::auth::ScramCredentials;
pub use document_engine::commands::EndpointFacts;
pub use document_engine::documents::Volume;
pub use document_engine::SharedVolume;

/// The SigV4 signing name of this service, under which the register of the planes this
/// emulator serves in process records what answers at the address a cluster publishes
/// (`smithy_query(query="service_info")`: `sigv4Name: docdb-elastic`).
const SIGNING_NAME: &str = super::ARN_SERVICE;

/// The MongoDB release an elastic cluster's endpoint answers as, which decides the wire
/// version the handshake reports and what `buildInfo` carries.
///
/// Source: the MongoDB version Amazon DocumentDB elastic clusters are compatible with (5.0).
pub const MONGO_COMPATIBILITY_VERSION: &str = "5.0.0";

/// The authentication type whose `adminUserPassword` is the password itself, which is the one
/// a cluster's endpoint can check a client's proof against (`Auth`: `PLAIN_TEXT | SECRET_ARN`).
///
/// A cluster created with `SECRET_ARN` carries the ARN of a secret rather than a password, so
/// this emulator derives no credential for it and its endpoint authenticates nobody — never
/// the other way round, which would serve a cluster's documents to a session that presented
/// nothing.
pub const AUTH_PLAIN_TEXT: &str = "PLAIN_TEXT";

/// Everything ONE elastic cluster's endpoint serves from: its documents, which of its
/// collections are sharded and on which key, and the credential a session authenticates
/// against.
///
/// Cloneable, because the socket, the store and every open connection hold the same storage:
/// each member is behind its own handle, so a write one session makes is what the next session
/// reads.
#[derive(Clone, Default)]
pub struct ClusterStorage {
    /// The databases, collections and documents this cluster holds.
    pub documents: SharedVolume,
    /// The shard key of each sharded collection ([`sharding`]).
    pub shard_keys: Arc<Mutex<sharding::ShardKeys>>,
    /// The administrator's SCRAM credentials, derived from the password the control plane
    /// holds and replaced when `UpdateCluster` changes it. `None` for a cluster whose
    /// authentication type names a secret rather than carrying a password.
    pub credentials: Arc<Mutex<Option<ScramCredentials>>>,
}

/// Written by hand rather than derived, so nothing that formats this storage can print a
/// value derived from the administrator's password.
impl std::fmt::Debug for ClusterStorage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClusterStorage")
            .field("documents", &self.documents)
            .field("shard_keys", &self.shard_keys)
            .finish_non_exhaustive()
    }
}

impl ClusterStorage {
    /// The cluster's documents, locked — recovering from a poisoned lock the way the engine's
    /// own state handle does, so one session that panicked does not wedge the cluster for
    /// every later client.
    pub fn lock_documents(&self) -> std::sync::MutexGuard<'_, Volume> {
        document_engine::lock_volume(&self.documents)
    }

    /// The shard keys, locked, with the same recovery.
    pub fn lock_shard_keys(&self) -> std::sync::MutexGuard<'_, sharding::ShardKeys> {
        self.shard_keys
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Record the credential this cluster's endpoint authenticates a session against, which
    /// the create derives from the administrator's password and a change of that password
    /// replaces.
    pub fn set_credentials(&self, credentials: Option<ScramCredentials>) {
        *self
            .credentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = credentials;
    }

    /// That credential as it is now.
    #[must_use]
    pub fn credentials(&self) -> Option<ScramCredentials> {
        self.credentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The credential a cluster's endpoint authenticates a session against: derived from the
/// administrator's name and the password the control plane holds, and `None` for a cluster
/// whose `authType` names a secret instead of carrying one ([`AUTH_PLAIN_TEXT`]).
///
/// The password is used only here: what the storage keeps is the salted, digested form, from
/// which the password cannot be recovered.
#[must_use]
pub fn credentials_for(
    auth_type: &str,
    admin_user_name: &str,
    admin_user_password: &str,
) -> Option<ScramCredentials> {
    match auth_type.eq_ignore_ascii_case(AUTH_PLAIN_TEXT) {
        true => Some(ScramCredentials::derive(
            admin_user_name,
            admin_user_password,
        )),
        false => None,
    }
}

/// WHERE the facts a session is judged by come from: this service's own store, read on every
/// command, so a change the control plane makes while a session is open — the cluster stopped,
/// the cluster deleted — is in force on that session's next command.
pub type FactsSource = Arc<dyn Fn() -> EndpointFacts + Send + Sync + 'static>;

/// What every session on ONE cluster's socket is served from.
#[derive(Clone)]
pub struct ClusterSocket {
    /// The endpoint's facts, read on every command.
    pub facts: FactsSource,
    /// The cluster's documents, shard keys and credential.
    pub storage: ClusterStorage,
    /// The id the next connection on this socket reports as its `connectionId`.
    pub next_connection_id: Arc<AtomicI64>,
}

impl std::fmt::Debug for ClusterSocket {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClusterSocket")
            .field("storage", &self.storage)
            .finish_non_exhaustive()
    }
}

impl ClusterSocket {
    /// The `connectionId` the next connection on this socket reports.
    #[must_use]
    pub fn take_connection_id(&self) -> i64 {
        self.next_connection_id.fetch_add(1, Ordering::Relaxed)
    }

    /// The endpoint as it is at this instant.
    #[must_use]
    pub fn facts(&self) -> EndpointFacts {
        (self.facts)()
    }
}

/// What a connection on an elastic cluster's socket is served from: this plane's own session,
/// and this plane's own command layer over the shared document engine.
impl document_engine::wire::WireEndpoint for ClusterSocket {
    type Session = session::ClusterSession;

    fn open_session(&self) -> Self::Session {
        session::ClusterSession::new(self.take_connection_id())
    }

    fn answer(
        &self,
        session: &mut Self::Session,
        database: &str,
        command: &::bson::Document,
    ) -> ::bson::Document {
        commands::answer(self, session, database, command)
    }

    fn carries_on(&self, session: &Self::Session) -> bool {
        session.carries_on()
    }
}

/// One cluster's data plane, as this store holds it.
///
/// Dropping it closes the port, so it is held for exactly as long as the cluster it belongs
/// to.
pub struct ClusterDataPlane {
    /// The `host:port` this cluster answers on, which is what `clusterEndpoint` publishes.
    endpoint: String,
    /// The documents, shard keys and credential the socket serves. Kept here so a plane that
    /// is re-opened for the same cluster serves what the cluster already held.
    storage: ClusterStorage,
    /// The socket itself. Never read: it is held because dropping it closes the port.
    _listener: ::emulator_server::ResourceListener,
    /// The record that makes the published address one an IN-PROCESS client of this emulator
    /// can open a session on — the same server as the socket's, over the same documents.
    /// Never read: dropping it withdraws the address, exactly as dropping the listener closes
    /// the port. `None` on an engine that vends no such register.
    _dialable: Option<::service_common::dialable_data_planes::OpenedPlanes>,
}

/// Written by hand rather than derived: a `ResourceListener` is a live socket and carries no
/// `Debug`. What is diagnosable is the address the plane answers at.
impl std::fmt::Debug for ClusterDataPlane {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClusterDataPlane")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl ClusterDataPlane {
    /// The `host:port` a client reaches this cluster at.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The documents, shard keys and credential it serves.
    #[must_use]
    pub fn storage(&self) -> &ClusterStorage {
        &self.storage
    }
}

/// The sockets this emulator has open for its elastic clusters, keyed by the `clusterArn` of
/// the cluster each serves.
///
/// Kept beside the cluster records rather than on them, because a record is a cloneable value
/// and a live socket is not.
#[derive(Debug, Default)]
pub struct ClusterDataPlanes {
    open: BTreeMap<String, ClusterDataPlane>,
}

impl ClusterDataPlanes {
    /// The data plane of the cluster `cluster_arn`, if this emulator opened one.
    #[must_use]
    pub fn get(&self, cluster_arn: &str) -> Option<&ClusterDataPlane> {
        self.open.get(cluster_arn)
    }

    /// Hold `plane` as the data plane of the cluster `cluster_arn`, closing whatever was held
    /// for it before.
    pub fn insert(&mut self, cluster_arn: &str, plane: ClusterDataPlane) {
        self.open.insert(cluster_arn.to_string(), plane);
    }

    /// Close the data plane of the cluster `cluster_arn`: the port and every connection on it
    /// go with the handle, and so do that cluster's documents.
    pub fn close(&mut self, cluster_arn: &str) -> Option<ClusterDataPlane> {
        self.open.remove(cluster_arn)
    }

    /// How many sockets are open, which is what bounds this map: one per served cluster, and a
    /// store holds at most [`super::MAX_CLUSTERS`] of those.
    #[must_use]
    pub fn len(&self) -> usize {
        self.open.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }
}

/// The facts of the cluster `cluster_arn`, read under this service's own store lock.
///
/// The transition the clock already owes the cluster is materialised first
/// ([`super::ClusterRecord::settle_reported`]), exactly as a reporting operation does: a
/// cluster whose `CREATING` -> `ACTIVE` move fell due is serving to a session too, whether or
/// not a control-plane read noticed it.
fn cluster_facts(
    state: &::engine::ServiceState<super::ComAmazonawsDocdbelasticChimeradblionfishservicelambdaState>,
    cluster_arn: &str,
    storage: &ClusterStorage,
) -> EndpointFacts {
    let mut store = state.lock();
    let Some(record) = store.clusters.get_mut(cluster_arn) else {
        return EndpointFacts {
            exists: false,
            serving: false,
            status: super::STATUS_DELETING.to_string(),
            is_writer: false,
            engine_version: MONGO_COMPATIBILITY_VERSION.to_string(),
            credentials: None,
        };
    };
    record.settle_reported();
    let status = record.status.clone();
    EndpointFacts {
        exists: true,
        // Only a cluster the control plane reports ACTIVE has an engine to answer out of: one
        // still being built, or stopped, serves no document command.
        serving: status == super::STATUS_ACTIVE,
        status,
        // An elastic cluster is reached at ONE endpoint, which is the one every write goes
        // through: there is no reader endpoint to serve reads only.
        is_writer: true,
        engine_version: MONGO_COMPATIBILITY_VERSION.to_string(),
        credentials: storage.credentials(),
    }
}

/// Open the socket one cluster is served on, and report the address it answers at — `None`
/// when this emulator's engine vends no listeners or the core could not bind.
///
/// `storage` is what the socket serves: the cluster's own documents, shard keys and
/// credential.
///
/// The wire is TLS from the first byte (`engine.listeners().tcp`), which is what a client of
/// an elastic cluster opens. The core owns the socket, the bind interface, the port range, the
/// certificate and the connection caps.
pub async fn open_cluster_data_plane<Eng: Send + Sync + 'static>(
    engine_handle: &Eng,
    facts: FactsSource,
    storage: ClusterStorage,
) -> Option<ClusterDataPlane> {
    let shared =
        (engine_handle as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()?;
    let listeners = shared.listeners();
    // ONE server per cluster, served at BOTH ends of its address: the socket the core accepts
    // a client's connections on, and the connections an in-process client of this emulator
    // opens to the address the control plane publishes.
    let served = Arc::new(document_engine::wire::MongoWire::new(ClusterSocket {
        facts,
        storage: storage.clone(),
        next_connection_id: Arc::new(AtomicI64::new(1)),
    }));
    // The handshake an in-process connection meets at byte 0 is the same one the socket
    // meets: the core terminates the TLS of this endpoint, so the core's own server runs the
    // handshake on the pipe too. An emulator whose TLS the core could not prepare serves the
    // SOCKET only, because a plane recorded for an address a client opens a handshake on would
    // answer that handshake with protocol bytes instead of failing cleanly.
    let upgrader = listeners.tls_upgrader().ok();
    let in_process = upgrader.is_some();
    let handler = ::service_common::dialable_data_planes::SharedHandler(Arc::clone(&served));
    // Opened on a task of its OWN, so a runtime that can bind no socket at all — one built
    // with no IO driver, which is what a host driving this emulator in process may hand a
    // request — leaves this create without an address of its own instead of taking the request
    // down with it.
    let opened = ::tokio::spawn(async move { listeners.tcp(handler).await.ok() }).await;
    let listener = opened.ok().flatten()?;
    // The address the CORE says the listener is dialable at, read through the one shared
    // reading of it: a wildcard bind has no single address to dial and is published as
    // loopback, and an IPv6 bind keeps its brackets.
    let authority = ::service_common::data_plane_addressing::listener_authority(&listener);
    let dialable = in_process
        .then(|| ::service_common::dialable_data_planes::DialablePlanes::of(engine_handle))
        .flatten()
        .map(|planes| {
            let plane = ::service_common::dialable_data_planes::served_in_process(served, upgrader);
            planes.opening(vec![authority.clone()], SIGNING_NAME, &plane)
        });
    Some(ClusterDataPlane {
        endpoint: authority,
        storage,
        _listener: listener,
        _dialable: dialable,
    })
}

/// Open the socket the cluster `cluster_arn` will be served on, BEFORE that cluster's record
/// is stored, and answer the plane the caller then holds on the store beside the record.
///
/// What a cluster publishes as its `clusterEndpoint` is the address of this plane
/// ([`ClusterDataPlane::endpoint`]), so the create reads it off the plane and stores it on the
/// record it mints: no cluster is ever stored carrying an address nothing answers at. A call
/// that ends up minting no cluster at all (a refusal under the store's lock) drops the plane
/// unused, which closes the port with it.
///
/// The credential the endpoint authenticates a session against is derived here from what the
/// request carried, because the record does not exist yet ([`credentials_for`]); the facts a
/// command is judged by are read from the store under the cluster's ARN on every command, so
/// until the record is there the endpoint reports itself as serving nothing.
///
/// `None` when this emulator's engine vends no listeners or the core could not bind, which the
/// caller answers with a modelled failure rather than by storing a cluster it cannot serve.
pub async fn open_cluster_plane<Eng: Send + Sync + 'static>(
    engine_handle: &Eng,
    state: &::engine::ServiceState<super::ComAmazonawsDocdbelasticChimeradblionfishservicelambdaState>,
    cluster_arn: &str,
    auth_type: &str,
    admin_user_name: &str,
    admin_user_password: &str,
) -> Option<ClusterDataPlane> {
    let storage = ClusterStorage::default();
    storage.set_credentials(credentials_for(
        auth_type,
        admin_user_name,
        admin_user_password,
    ));
    let facts: FactsSource = {
        let state = state.clone();
        let cluster_arn = cluster_arn.to_string();
        let storage = storage.clone();
        Arc::new(move || cluster_facts(&state, &cluster_arn, &storage))
    };
    open_cluster_data_plane(engine_handle, facts, storage).await
}

/// Re-derive the credential the cluster `cluster_arn` authenticates a session against, from
/// the administrator name and password its record holds NOW.
///
/// What needs it is `UpdateCluster`: a changed `adminUserPassword` is the password a later
/// session has to present, and a changed `authType` decides whether there is a password to
/// present at all. Sessions already open are judged by the credential on their next command,
/// because the facts are read per command.
pub fn refresh_credentials(
    store: &super::ComAmazonawsDocdbelasticChimeradblionfishservicelambdaState,
    cluster_arn: &str,
) {
    let Some(plane) = store.cluster_planes.get(cluster_arn) else {
        return;
    };
    let Some(record) = store.clusters.get(cluster_arn) else {
        return;
    };
    plane.storage().set_credentials(credentials_for(
        &record.auth_type,
        &record.admin_user_name,
        &record.admin_user_password,
    ));
}
