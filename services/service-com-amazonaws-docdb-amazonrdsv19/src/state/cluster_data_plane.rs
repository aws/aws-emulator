// SPDX-License-Identifier: CC0-1.0

//! The DATA PLANE of a cluster: the address a client of an Amazon DocumentDB instance
//! connects to, the documents behind it, and what the control plane publishes for it.
//!
//! An endpoint a service advertises is an address the service ANSWERS on: `DBInstance$Endpoint`
//! is "the connection endpoint", whose `Address` is "the DNS address of the instance" and
//! whose `Port` is "the port that the database engine is listening on", and the structure's
//! own documentation says what a client does with the pair — "Client programs must specify a
//! valid endpoint to access these Amazon DocumentDB resources." A name under Amazon
//! DocumentDB's own DNS zone resolves to nothing on the machine this emulator runs on, so an
//! instance that reported one published an address no client could reach.
//!
//! So every endpoint this emulator publishes for a cluster whose engine speaks the document
//! wire ([`engine_speaks_the_document_wire`]) — the CLUSTER's own two endpoints and each of its
//! members' — is given a socket of the EMULATOR's own, opened through the core
//! (`engine.listeners()`, which owns the socket, the bind interface, the port range, the TLS
//! material and the connection caps), and what `DBCluster{Endpoint,ReaderEndpoint}` and an
//! instance's `Endpoint{Address,Port}` report is an address that socket answers at. A client
//! dials exactly what it was told, and what answers there is the server [`wire`] runs over the
//! documents [`documents`] holds.
//!
//! `DBCluster` gives the cluster's two endpoint members no port of their own — one `Port`
//! member serves both — so a client completes the published host with that `Port`, which is
//! the port the cluster's engine was configured to listen on rather than the one the core
//! assigned its socket. The address a client composes that way is therefore recorded BESIDE
//! the socket's own, so the pair the control plane publishes is one this emulator answers at
//! ([`open_endpoint_data_plane`]).
//!
//! WHICH address of that socket is published is the one the CORE says it is dialable at, with
//! the port the core assigned: a socket is bound on one address and answers a connection
//! addressed to that address alone, so what an instance publishes is the socket's own
//! `host:port` and nothing else. Each member of a cluster is therefore reached at an endpoint
//! of its own, because each has a socket of its own. That address is also recorded as one this
//! emulator serves (`service_common::dialable_data_planes`), so a client of this emulator that
//! cannot open a host socket — the code of a function attached to the cluster's virtual private
//! cloud, which is the vantage point such an endpoint is reachable from on AWS — reaches the
//! same server at the same address.
//!
//! What each part owns:
//!
//! * this module — the endpoint record the store holds per instance, which engine speaks the
//!   wire, whether that wire is encrypted, and the facts a session is judged against;
//! * [`wire`] — the engine's message framing, one session per accepted connection;
//! * [`commands`] — the commands a session may send and what each is answered with;
//! * [`auth`] — the SASL exchange that authenticates a session as the cluster's master user;
//! * [`documents`] — the databases, collections, indexes and documents one cluster holds;
//! * [`query`] — how a filter selects documents, how a sort orders them and what an update
//!   changes;
//! * [`fault`] — how a command is refused, in the form the engine's clients read.
//!
//! The socket's life is the instance's: the handle lives on the store under the instance's own
//! key, and dropping it (the delete) closes the port and severs every connection on it. The
//! DOCUMENTS outlive it, because they are the CLUSTER's — "each Amazon DocumentDB cluster
//! hosts copies of its data" — so every member of one cluster is a second address onto one
//! volume, and the volume goes when the cluster does.

pub mod auth;
pub mod commands;
pub mod documents;
pub mod fault;
pub mod query;
pub mod volumes;
pub mod wire;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use commands::EndpointFacts;
use documents::Volume;

pub use volumes::ClusterDocuments;

/// The SigV4 signing name of the front door this service is a view of, which the register of
/// in-process planes records as the service answering at an address this service opened
/// (`smithy_query(query="service_info")`: `sigv4Name: rds`, shared by the three models of
/// that door).
const SIGNING_NAME: &str = "rds";

/// The value of the cluster parameter `tls` that turns the endpoint's encryption OFF. Every
/// other value of it leaves the endpoint encrypted, which is that parameter's default
/// (`service_common::rds_cluster_parameters`: `tls`, "TLS Settings", default `enabled`).
pub const TLS_DISABLED: &str = "disabled";

/// The name of the cluster parameter that decides whether the endpoint is encrypted.
pub const TLS_PARAMETER: &str = "tls";

/// The address one instance's own socket answers on, as the control plane publishes it.
///
/// The port comes from the core listener and the host from the interface the core bound it on,
/// never from a `format!` of a service hostname: it is an address a client of this emulator
/// can reach the instance at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedEndpoint {
    pub host: String,
    pub port: i32,
}

/// One cluster's documents, shared by its members' sockets and held by the cluster.
pub type SharedVolume = Arc<Mutex<Volume>>;

/// WHERE the facts a session is judged by come from: the control-plane store of the view that
/// opened the endpoint, read on every command.
///
/// A closure rather than one store's reader, because the SAME wire is served for a cluster
/// either view of this front door created ([`volumes`]): the DocumentDB model reads its own
/// records and the Amazon RDS model of the same door reads its own, and what the wire needs of
/// either is this one answer.
pub type FactsSource = Arc<dyn Fn() -> EndpointFacts + Send + Sync + 'static>;

/// Lock a volume, recovering from a poisoned lock the way the engine's own state handle does:
/// one session that panicked must not wedge the cluster for every later client.
pub fn lock_volume(volume: &SharedVolume) -> std::sync::MutexGuard<'_, Volume> {
    volume.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What every session on ONE endpoint's socket is served from — a cluster's own endpoints and
/// each of its members'.
#[derive(Clone)]
pub struct EndpointSocket {
    /// The endpoint's facts, read on every command, so a change the control plane makes while
    /// a session is open is in force on its next command.
    pub facts: FactsSource,
    /// The documents of the cluster this instance is a member of.
    pub volume: SharedVolume,
    /// The id the next connection on this socket reports as its `connectionId`.
    pub next_connection_id: Arc<AtomicI64>,
}

impl std::fmt::Debug for EndpointSocket {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("EndpointSocket").finish_non_exhaustive()
    }
}

impl EndpointSocket {
    /// The `connectionId` the next connection on this socket reports.
    #[must_use]
    pub fn take_connection_id(&self) -> i64 {
        self.next_connection_id.fetch_add(1, Ordering::Relaxed)
    }

    /// The cluster's documents, locked.
    pub fn lock_volume(&self) -> std::sync::MutexGuard<'_, Volume> {
        lock_volume(&self.volume)
    }

    /// The endpoint as it is at this instant.
    #[must_use]
    pub fn facts(&self) -> EndpointFacts {
        (self.facts)()
    }
}

/// What a connection on ONE endpoint's socket is served from: a session of the command layer
/// below ([`commands`]), and every command answered out of the cluster's documents with the
/// endpoint's facts as they are at that instant.
impl wire::WireEndpoint for EndpointSocket {
    type Session = commands::Session;

    fn open_session(&self) -> Self::Session {
        commands::Session::new(self.take_connection_id())
    }

    fn answer(
        &self,
        session: &mut Self::Session,
        database: &str,
        command: &::bson::Document,
    ) -> ::bson::Document {
        let facts = self.facts();
        let mut volume = self.lock_volume();
        commands::execute(session, &facts, &mut volume, database, command)
    }
}

/// The facts of an endpoint of the DocumentDB cluster the DOCDB store holds, read under that
/// store's own lock and the shared cluster storage's.
///
/// The transitions the clock already owes are materialised first
/// ([`super::ComAmazonawsDocdbAmazonrdsv19State::settle_clock`]): an instance whose
/// `creating` -> `available` transition fell due is available to a session too, whether or not
/// a control-plane read noticed it.
fn docdb_store_facts(
    state: &::engine::ServiceState<super::ComAmazonawsDocdbAmazonrdsv19State>,
    documents: Option<&::engine::ServiceState<ClusterDocuments>>,
    instance_identifier: &str,
) -> EndpointFacts {
    let mut store = state.lock();
    store.settle_clock();
    let Some(instance) = store.instances.get(instance_identifier) else {
        return EndpointFacts {
            exists: false,
            serving: false,
            status: super::STATUS_DELETING.to_string(),
            is_writer: false,
            engine_version: super::DEFAULT_ENGINE_VERSION.to_string(),
            credentials: None,
        };
    };
    let status = instance.status.clone();
    let cluster = store.clusters.get(&instance.cluster_identifier);
    EndpointFacts {
        exists: true,
        serving: status == super::STATUS_AVAILABLE,
        status,
        // Writes go to the member the cluster writes through, so a member that is not the
        // writer serves reads only — which a failover moves under the live sessions of both.
        is_writer: cluster.is_some_and(|cluster| {
            cluster.members.iter().any(|member| {
                member.is_cluster_writer && member.db_instance_identifier == instance_identifier
            })
        }),
        engine_version: cluster
            .map(|cluster| cluster.engine_version.clone())
            .unwrap_or_else(|| super::DEFAULT_ENGINE_VERSION.to_string()),
        credentials: cluster.and_then(|cluster| {
            documents?
                .lock()
                .credentials(&cluster.resource_id)
        }),
    }
}

/// ONE endpoint's data plane, as the store holds it.
///
/// Dropping it closes the port, so it is held for exactly as long as the resource it belongs
/// to.
pub struct ServedDataPlane {
    /// The address this endpoint answers on, which the control plane publishes.
    endpoint: ServedEndpoint,
    /// The socket itself. Never read: it is held because dropping it closes the port.
    _listener: ::emulator_server::ResourceListener,
    /// The record that makes the published address one an IN-PROCESS client of this emulator
    /// can open a session on — the same server as the socket's, over the same documents.
    /// Held because dropping it withdraws the addresses, exactly as dropping the listener
    /// closes the port, and replaced when the control plane moves the port it publishes beside
    /// this endpoint's host ([`ServedDataPlane::publishes_port`]). `None` on an engine that
    /// vends no such register, and for an endpoint served on its socket alone.
    dialable: Option<::service_common::dialable_data_planes::OpenedPlanes>,
    /// The server itself, as the register of in-process planes holds it — kept so the
    /// addresses it answers at can be re-recorded without re-opening the socket. `None` for an
    /// endpoint that is served on its socket alone.
    served: Option<::std::sync::Arc<dyn ::service_common::dialable_data_planes::DialablePlane>>,
}

impl std::fmt::Debug for ServedDataPlane {
    /// Everything about this record that is not a live socket: where it answers.
    /// `ResourceListener` is not `Debug`.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServedDataPlane")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl ServedDataPlane {
    /// The address a client reaches this endpoint at.
    #[must_use]
    pub fn endpoint(&self) -> &ServedEndpoint {
        &self.endpoint
    }

    /// Re-record the address a client COMPOSES from this endpoint's published host and the
    /// port the control plane publishes beside it, for a control plane that MOVED that port.
    ///
    /// The socket does not move — the core owns the port it bound, and a cluster's `Port` is
    /// not that port — so what changes is only the second address this one server answers at:
    /// the pair the caller is handed from now on. Synchronous, and the register it writes is
    /// the last lock in this service's order, so it is called with the store's guard held.
    /// Does nothing for an endpoint served on its socket alone.
    pub fn publishes_port<Eng: Send + Sync + 'static>(&mut self, engine: &Eng, port: i32) {
        let Some(served) = self.served.clone() else {
            return;
        };
        let Some(planes) = ::service_common::dialable_data_planes::DialablePlanes::of(engine)
        else {
            return;
        };
        let authorities = answered_authorities(&self.endpoint.host, self.endpoint.port, Some(port));
        // The new recording replaces the addresses it names; the handle held before is dropped
        // with the assignment, which withdraws only the addresses still holding ITS recording
        // — the port no longer published.
        self.dialable = Some(planes.opening(authorities, SIGNING_NAME, &served));
    }
}

/// The addresses ONE server answers at: the socket's own `host:port`, and — for an endpoint
/// whose published member carries no port of its own — that host completed with the port the
/// control plane publishes beside it, which is the address a client composes out of the two
/// members it was handed.
fn answered_authorities(host: &str, port: i32, published_port: Option<i32>) -> Vec<String> {
    let authority = |port: i32| {
        u16::try_from(port)
            .ok()
            .map(|port| ::service_common::data_plane_addressing::host_port_authority(host, port))
    };
    [
        authority(port),
        published_port.filter(|beside| *beside != port).and_then(authority),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The sockets this emulator has open for one KIND of resource, keyed by the resource each of
/// them serves — its instances by instance identifier, its clusters by the cluster's immutable
/// `DbClusterResourceId`, which a rename does not move.
///
/// Kept beside the records rather than on them, because a record is a cloneable value and a
/// live socket is not.
#[derive(Debug, Default)]
pub struct ServedDataPlanes {
    open: BTreeMap<String, ServedDataPlane>,
}

impl ServedDataPlanes {
    /// The data plane keyed `identifier`, if this emulator opened one.
    #[must_use]
    pub fn get(&self, identifier: &str) -> Option<&ServedDataPlane> {
        self.open.get(identifier)
    }

    /// The data plane keyed `identifier`, to re-record what it publishes
    /// ([`ServedDataPlane::publishes_port`]).
    pub fn get_mut(&mut self, identifier: &str) -> Option<&mut ServedDataPlane> {
        self.open.get_mut(identifier)
    }

    /// Hold `plane` as the data plane keyed `identifier`, closing whatever was held for it
    /// before.
    pub fn insert(&mut self, identifier: &str, plane: ServedDataPlane) {
        self.open.insert(identifier.to_string(), plane);
    }

    /// Close the data plane keyed `identifier`: the port and every connection on it go with
    /// the handle.
    pub fn close(&mut self, identifier: &str) -> Option<ServedDataPlane> {
        self.open.remove(identifier)
    }

    /// How many sockets are open, which is what bounds this map: one per served resource.
    #[must_use]
    pub fn len(&self) -> usize {
        self.open.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }
}

/// Whether this emulator serves a data plane for an instance of a cluster running `engine`.
///
/// `CreateDBClusterMessage$Engine`: "Valid values: docdb" — the one engine of this service,
/// whose clients speak the document wire [`wire`] serves. An engine outside that value gets no
/// listener, so nothing publishes a port that would answer the wrong protocol.
#[must_use]
pub fn engine_speaks_the_document_wire(engine: &str) -> bool {
    engine.eq_ignore_ascii_case(super::DOCDB_ENGINE)
}

/// Whether the endpoints of the cluster keyed `cluster_identifier` are ENCRYPTED.
///
/// The engine carries this as a cluster PARAMETER, so the answer is the effective value of
/// that parameter for this cluster: what the cluster's own parameter group sets, and the
/// parameter's documented default (`enabled`) where the group sets nothing — which is also the
/// answer for a cluster carrying a default group AWS provides, since such a group sets no
/// parameter of its own.
#[must_use]
pub fn endpoints_are_encrypted(
    store: &super::ComAmazonawsDocdbAmazonrdsv19State,
    cluster_identifier: &str,
) -> bool {
    let setting = store
        .clusters
        .get(cluster_identifier)
        .and_then(|cluster| cluster.db_cluster_parameter_group.as_ref())
        .and_then(|group| store.cluster_parameter_groups.get(group))
        .and_then(|group| group.parameters.get(TLS_PARAMETER))
        .map(|setting| setting.value.clone());
    !setting.is_some_and(|value| value.eq_ignore_ascii_case(TLS_DISABLED))
}

/// Open the socket one instance is served on, and report the address it answers at.
///
/// [`open_endpoint_data_plane`] for a resource whose published endpoint carries a port member
/// of its own (`Endpoint$Port`), so the address a client composes is the socket's own and no
/// second one is recorded for it.
pub async fn open_instance_data_plane<Eng: Send + Sync + 'static>(
    engine: &Eng,
    facts: FactsSource,
    volume: SharedVolume,
    encrypted: bool,
) -> Option<ServedDataPlane> {
    open_endpoint_data_plane(engine, facts, volume, encrypted, None).await
}

/// Open the socket ONE endpoint is served on, and report the address it answers at — `None`
/// when this engine vends no listeners or the core could not bind, in which case the caller
/// publishes the name Amazon DocumentDB's own zone would carry instead.
///
/// `volume` is the data the socket serves: its CLUSTER's, which is what makes a member answer
/// what another member wrote.
///
/// `encrypted` chooses the wire: an endpoint whose cluster has TLS on is served
/// TLS-from-the-first-byte, which is how a client of this engine connects to one
/// (`engine.listeners().tcp`), and one with TLS off is served in the clear
/// (`tcp_plaintext`). The core owns the socket, the bind interface, the port range, the
/// certificate and the connection caps in both cases.
///
/// The address the socket answers on is also recorded as one this emulator serves in process
/// (`service_common::dialable_data_planes`), so a client of this emulator that cannot open a
/// host socket — the code of a function attached to the cluster's virtual private cloud, which
/// is the vantage point a cluster endpoint is reachable from — opens its connection to the
/// published address and is served by this same server.
///
/// `published_port` is the port the control plane publishes BESIDE the host for an endpoint
/// whose own published member carries none — `DBCluster` gives its two endpoint members one
/// shared `Port`, the port the cluster's engine was configured to listen on — and the address
/// a client composes from the two is then recorded as a second address of this same server,
/// because it is the pair the caller was handed. `None` for an endpoint published as a whole
/// `host`/`port` pair of the socket's own, which needs no second address.
///
/// That configured port is ONE port of the ONE interface the core binds on, so it names no
/// cluster in particular: the cluster that published its endpoints last answers there, while
/// each endpoint's own socket address keeps reaching its own endpoint throughout. Giving every
/// cluster an address of its own to answer that port at needs a core listener that takes an
/// address or a port, and `vendor/server/src/listeners.rs` takes neither.
pub async fn open_endpoint_data_plane<Eng: Send + Sync + 'static>(
    engine: &Eng,
    facts: FactsSource,
    volume: SharedVolume,
    encrypted: bool,
    published_port: Option<i32>,
) -> Option<ServedDataPlane> {
    let shared = (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()?;
    let listeners = shared.listeners();
    // ONE server per endpoint, served at BOTH ends of its address: the socket the core accepts
    // a client's connections on, and the connections an in-process client of this emulator
    // opens to the address the control plane publishes.
    let served = Arc::new(wire::MongoWire::new(EndpointSocket {
        facts,
        volume,
        next_connection_id: Arc::new(AtomicI64::new(1)),
    }));
    // The handshake an in-process connection meets at byte 0: an encrypted endpoint is
    // terminated by the CORE on its socket, so the core's own server runs the handshake on the
    // pipe too; an endpoint served in the clear negotiates nothing.
    let upgrader = match encrypted {
        true => listeners.tls_upgrader().ok(),
        false => None,
    };
    // An endpoint whose TLS the core could not prepare at all is served on its SOCKET only: a
    // plaintext plane recorded for an address a client opens a handshake on would answer that
    // handshake with protocol bytes instead of failing cleanly.
    let in_process = !encrypted || upgrader.is_some();
    let handler = ::service_common::dialable_data_planes::SharedHandler(Arc::clone(&served));
    // Opened on a task of its OWN, so a runtime that cannot bind a socket at all — one built
    // with no IO driver, which is what a host driving this emulator in process may hand a
    // request — leaves this create without an address of its own instead of taking the request
    // down with it.
    let opened = ::tokio::spawn(async move {
        match encrypted {
            true => listeners.tcp(handler).await.ok(),
            false => listeners.tcp_plaintext(handler).await.ok(),
        }
    })
    .await;
    let listener = opened.ok().flatten()?;
    // The address the CORE says the listener is dialable at, read through the one shared
    // reading of it: a wildcard bind has no single address to dial and is published as
    // loopback, and an IPv6 bind keeps its brackets.
    let authority = ::service_common::data_plane_addressing::listener_authority(&listener);
    let host = ::service_common::data_plane_addressing::host_only(&authority).to_string();
    let port = ::service_common::data_plane_addressing::addressed_port(Some(&authority))
        .unwrap_or_else(|| i32::from(listener.port()));
    // WHICH address of that socket the control plane publishes: the one the CORE says the
    // socket is dialable at, and the PORT it assigned — so a client that dials what it was
    // told reaches the socket the core really opened. A socket is bound on one address, and
    // answers a connection addressed to that address alone, so nothing else is published for
    // it: each member of a cluster is reached at an endpoint of its own because each has a
    // socket of its own, on its own port.
    let published = ServedEndpoint {
        host: host.clone(),
        port,
    };
    // The addresses this one server answers at ([`answered_authorities`]): the socket's own,
    // and — for an endpoint whose published member carries no port of its own — that host
    // completed with the port the control plane publishes beside it.
    let authorities = answered_authorities(&host, port, published_port);
    // The server as the register of in-process planes holds it, kept on the record so the
    // addresses it answers at can be re-recorded when the control plane moves the port it
    // publishes ([`ServedDataPlane::publishes_port`]).
    let served = in_process
        .then(|| ::service_common::dialable_data_planes::served_in_process(served, upgrader));
    let dialable = served.clone().and_then(|served| {
        ::service_common::dialable_data_planes::DialablePlanes::of(engine)
            .map(|planes| planes.opening(authorities, SIGNING_NAME, &served))
    });
    Some(ServedDataPlane {
        endpoint: published,
        _listener: listener,
        dialable,
        served,
    })
}

/// Open the socket the instance keyed `instance_identifier` is served on and publish its
/// address on the instance's record, so the endpoint every `DBInstance`-returning operation
/// reports is one a client can reach.
///
/// Called by the create once the record is stored, with the store NOT locked: opening a socket
/// is asynchronous and the store's lock is held across no await anywhere in this service.
/// Answers the address published, or `None` for an instance of a cluster whose engine speaks no
/// wire this emulator serves and for an engine that vends no listeners — in which case the
/// instance keeps the name Amazon DocumentDB's own zone would carry.
pub async fn serve_instance<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<super::ComAmazonawsDocdbAmazonrdsv19State>,
    instance_identifier: &str,
) -> Option<ServedEndpoint> {
    let documents = ClusterDocuments::of(engine);
    // Everything read from the store is read before the await, and the volume handle is the
    // CLUSTER's, so two members of one cluster are two addresses onto one set of documents.
    let (volume, encrypted) = {
        let store = state.lock();
        let instance = store.instances.get(instance_identifier)?;
        let cluster_identifier = instance.cluster_identifier.clone();
        let cluster = store.clusters.get(&cluster_identifier)?;
        if !engine_speaks_the_document_wire(&cluster.engine) {
            return None;
        }
        let resource_id = cluster.resource_id.clone();
        let encrypted = endpoints_are_encrypted(&store, &cluster_identifier);
        // The cluster's storage is taken under this service's own store, which is the lock
        // order [`volumes`] fixes.
        (documents.as_ref()?.lock().volume(&resource_id), encrypted)
    };
    let facts: FactsSource = {
        let state = state.clone();
        let documents = documents.clone();
        let instance_identifier = instance_identifier.to_string();
        Arc::new(move || {
            docdb_store_facts(&state, documents.as_ref(), &instance_identifier)
        })
    };
    let plane = open_instance_data_plane(engine, facts, volume, encrypted).await?;
    let endpoint = plane.endpoint().clone();
    let mut store = state.lock();
    // The record publishes what the socket answers on, so every later read of the instance
    // reports the same address — and the handle is held beside it, because dropping it closes
    // the port.
    //
    // The lock was released while the socket was opened, so the record is read back fallibly: a
    // delete that ran in between removed the instance and found no plane to close, so there is
    // nothing to publish here and the socket closes with the plane as this returns rather than
    // being held open under a key no record has.
    let instance = store.instances.get_mut(instance_identifier)?;
    instance.served_endpoint = Some(endpoint.clone());
    store
        .instance_data_planes
        .insert(instance_identifier, plane);
    Some(endpoint)
}

/// The facts of a CLUSTER's own endpoint, read under this service's store and the shared
/// cluster storage's.
///
/// The cluster is found by its immutable `DbClusterResourceId` rather than by its identifier,
/// because `ModifyDBCluster` renames a cluster under the live sessions of its endpoints and the
/// resource id is what does not move.
///
/// `DBCluster$Endpoint` is the connection endpoint for the cluster's PRIMARY instance, so what
/// answers here answers as that primary: writes are accepted, and the endpoint serves only
/// while the cluster is available and the member it writes through is — a cluster with no
/// member yet has no engine behind its endpoint, exactly as one still being created has none.
fn docdb_cluster_facts(
    state: &::engine::ServiceState<super::ComAmazonawsDocdbAmazonrdsv19State>,
    documents: Option<&::engine::ServiceState<ClusterDocuments>>,
    resource_id: &str,
) -> EndpointFacts {
    let mut store = state.lock();
    store.settle_clock();
    let Some(cluster) = store
        .clusters
        .values()
        .find(|cluster| cluster.resource_id == resource_id)
    else {
        return EndpointFacts {
            exists: false,
            serving: false,
            status: super::STATUS_DELETING.to_string(),
            is_writer: false,
            engine_version: super::DEFAULT_ENGINE_VERSION.to_string(),
            credentials: None,
        };
    };
    // The member the cluster writes through, which is the instance this endpoint leads to.
    let writer = cluster
        .members
        .iter()
        .find(|member| member.is_cluster_writer)
        .and_then(|member| store.instances.get(&member.db_instance_identifier));
    EndpointFacts {
        exists: true,
        serving: cluster.status == super::STATUS_AVAILABLE
            && writer.is_some_and(|instance| instance.status == super::STATUS_AVAILABLE),
        // Why a session is not served, when it is not: the primary's own status where the
        // cluster has one, and the cluster's own while it has no member to lead to.
        status: writer.map_or_else(
            || cluster.status.clone(),
            |instance| instance.status.clone(),
        ),
        is_writer: true,
        engine_version: cluster.engine_version.clone(),
        credentials: documents.and_then(|documents| {
            documents.lock().credentials(&cluster.resource_id)
        }),
    }
}

/// Open the socket the CLUSTER keyed `cluster_identifier` is served on and publish its address
/// on the cluster's record, so `DBCluster$Endpoint` and `DBCluster$ReaderEndpoint` are an
/// address a client can reach rather than a name in Amazon DocumentDB's own DNS zone.
///
/// Called by the create once the record is stored, and again by a modify that MOVES the port
/// the cluster publishes — the address a client composes from the published host and `Port`
/// changes with it, so the server is re-opened to answer at the new one. Both call it with the
/// store NOT locked: opening a socket is asynchronous and the store's lock is held across no
/// await anywhere in this service. Whatever was open for the cluster closes as the new plane
/// replaces it, which is what a port change does to the sessions on the old port.
///
/// Answers the address published, or `None` for a cluster whose engine speaks no wire this
/// emulator serves and for an engine that vends no listeners — in which case the cluster keeps
/// the names Amazon DocumentDB's own zone would carry.
pub async fn serve_cluster<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<super::ComAmazonawsDocdbAmazonrdsv19State>,
    cluster_identifier: &str,
) -> Option<ServedEndpoint> {
    let documents = ClusterDocuments::of(engine);
    // Everything read from the store is read before the await, and the volume handle is the
    // CLUSTER's, so the cluster's endpoint and its members' are addresses onto one set of
    // documents.
    let (resource_id, volume, encrypted, published_port) = {
        let store = state.lock();
        let cluster = store.clusters.get(cluster_identifier)?;
        if !engine_speaks_the_document_wire(&cluster.engine) {
            return None;
        }
        let resource_id = cluster.resource_id.clone();
        let published_port = cluster.port;
        let encrypted = endpoints_are_encrypted(&store, cluster_identifier);
        // The cluster's storage is taken under this service's own store, which is the lock
        // order [`volumes`] fixes.
        (
            resource_id.clone(),
            documents.as_ref()?.lock().volume(&resource_id),
            encrypted,
            published_port,
        )
    };
    let facts: FactsSource = {
        let state = state.clone();
        let documents = documents.clone();
        let resource_id = resource_id.clone();
        Arc::new(move || docdb_cluster_facts(&state, documents.as_ref(), &resource_id))
    };
    let plane =
        open_endpoint_data_plane(engine, facts, volume, encrypted, Some(published_port)).await?;
    let endpoint = plane.endpoint().clone();
    let mut store = state.lock();
    // The record publishes what the socket answers on, so every later read of the cluster
    // reports the same host.
    //
    // The lock was released while the socket was opened, so the record is read back fallibly: a
    // delete that ran in between removed the cluster, so there is nothing to publish here and
    // the socket closes with the plane as this returns rather than being held open for a
    // cluster no record has.
    let cluster = store.clusters.get_mut(cluster_identifier)?;
    cluster.served_endpoint = Some(endpoint.clone());
    // Keyed by the resource id, which a rename does not move, so the socket of a renamed
    // cluster is still the socket that cluster's delete closes.
    store.cluster_data_planes.insert(&resource_id, plane);
    Some(endpoint)
}
